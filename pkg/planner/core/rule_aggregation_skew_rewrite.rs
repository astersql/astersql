// Copyright 2022 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 倾斜 DISTINCT 聚合改写规则。
//
// 当分组聚合恰有一个 COUNT(DISTINCT) 且其余聚合可分解时，拆成底层按
// 「原分组 + DISTINCT 参数」分组、上层再聚合，缓解数据倾斜。

use crate::rule_aggregation_elimination::{
    AggFuncDesc, AggFuncName, AggMode, LogicalAggregation, LogicalPlan, Result,
};
use crate::task::Expression;
use std::collections::HashSet;

/// 倾斜 DISTINCT 聚合改写器；分配合成列 ID。
#[derive(Default)]
pub struct SkewDistinctAggRewriter {
    next_column_id: usize,
}
impl SkewDistinctAggRewriter {
    /// 尝试把合格聚合改写成 Bottom+Top 两层聚合（必要时再加投影）。
    pub fn rewriteSkewDistinctAgg(&mut self, agg: &LogicalAggregation) -> Option<LogicalPlan> {
        if agg.group_by_items.is_empty() {
            return None;
        }
        let distinct: Vec<_> = agg
            .agg_funcs
            .iter()
            .enumerate()
            .filter(|(_, f)| f.distinct)
            .collect();
        // Go 侧当前仅支持恰好一个、单参数的 DISTINCT 聚合。
        if distinct.len() != 1
            || distinct[0].1.args.len() != 1
            || agg.agg_funcs.iter().any(|f| !self.isQualifiedAgg(f))
        {
            return None;
        }
        let distinct_arg = distinct[0].1.args[0].clone();
        // Go 侧直接追加 DISTINCT 参数，即使它已是原分组项也保留。
        let mut bottom_group = agg.group_by_items.clone();
        bottom_group.push(distinct_arg);
        let mut first_row_columns: HashSet<_> =
            agg.group_by_items.iter().filter_map(|e| e.column).collect();
        let mut bottom_funcs = Vec::new();
        let mut top_funcs = Vec::new();
        let mut projection_needed = false;
        for function in &agg.agg_funcs {
            if function.distinct {
                let mut first = function.clone();
                first.name = AggFuncName::FirstRow;
                first.distinct = false;
                first.mode = AggMode::Complete;
                bottom_funcs.push(first);
                let mut top = function.clone();
                top.distinct = false;
                top.mode = AggMode::Complete;
                top.args = vec![synthetic_column(
                    &mut self.next_column_id,
                    &function.return_type,
                )];
                top_funcs.push(top);
                continue;
            }
            let mut bottom = function.clone();
            bottom.distinct = false;
            bottom_funcs.push(bottom);
            let output = synthetic_column(&mut self.next_column_id, &function.return_type);
            let mut top = function.clone();
            top.args = vec![output];
            // Partial COUNT 上层用 SUM 合并；最终需投影 cast 回 COUNT 类型。
            if function.name == AggFuncName::Count {
                top.name = AggFuncName::Sum;
                projection_needed = true;
            } else if function.name == AggFuncName::FirstRow {
                if let Some(column) = function.args.first().and_then(|arg| arg.column) {
                    first_row_columns.remove(&column);
                }
            }
            top_funcs.push(top);
        }
        for group in &agg.group_by_items {
            if group.column.is_some_and(|c| first_row_columns.contains(&c)) {
                let mut first = agg.agg_funcs[0].clone();
                first.name = AggFuncName::FirstRow;
                first.args = vec![group.clone()];
                first.distinct = false;
                first.mode = AggMode::Complete;
                bottom_funcs.push(first);
            }
        }
        let bottom = LogicalPlan::Aggregation(LogicalAggregation {
            agg_funcs: bottom_funcs,
            group_by_items: bottom_group,
            child: agg.child.clone(),
            schema: agg.schema.clone(),
            output_columns: agg.output_columns.clone(),
            no_eliminate: true,
        });
        let top = LogicalPlan::Aggregation(LogicalAggregation {
            agg_funcs: top_funcs,
            group_by_items: agg.group_by_items.clone(),
            child: Box::new(bottom),
            schema: agg.schema.clone(),
            output_columns: agg.output_columns.clone(),
            no_eliminate: true,
        });
        if projection_needed {
            let expressions = agg
                .output_columns
                .iter()
                .enumerate()
                .map(|(offset, column)| Expression {
                    name: if agg.agg_funcs[offset].name == AggFuncName::Count {
                        format!("cast(col_{column})")
                    } else {
                        format!("col_{column}")
                    },
                    column: Some(*column),
                    return_type: agg.schema.get(offset).cloned(),
                    ..Expression::default()
                })
                .collect();
            Some(LogicalPlan::Projection {
                expressions,
                child: Box::new(top),
                schema: agg.schema.clone(),
            })
        } else {
            Some(top)
        }
    }
    /// 聚合是否适合本改写：Complete 模式、简单参数与 Go 白名单。
    pub fn isQualifiedAgg(&self, function: &AggFuncDesc) -> bool {
        if function.mode != AggMode::Complete
            || !function.order_by.is_empty()
            || function.args.len() > 1
            || function
                .args
                .iter()
                .any(|arg| arg.column.is_none() && arg.function_count != 0)
        {
            return false;
        }
        match function.name {
            AggFuncName::FirstRow
            | AggFuncName::Count
            | AggFuncName::Sum
            | AggFuncName::Max
            | AggFuncName::Min => true,
            AggFuncName::Avg => function.distinct,
            AggFuncName::BitAnd | AggFuncName::BitOr | AggFuncName::BitXor => false,
            _ => false,
        }
    }
    /// 递归优化；在聚合节点尝试倾斜 DISTINCT 改写。
    pub fn Optimize(&mut self, plan: LogicalPlan) -> Result<(LogicalPlan, bool)> {
        match plan {
            LogicalPlan::Aggregation(mut agg) => {
                let (child, child_changed) = self.Optimize(*agg.child)?;
                agg.child = Box::new(child);
                if let Some(rewritten) = self.rewriteSkewDistinctAgg(&agg) {
                    Ok((rewritten, child_changed))
                } else {
                    Ok((LogicalPlan::Aggregation(agg), child_changed))
                }
            }
            LogicalPlan::Projection {
                expressions,
                child,
                schema,
            } => {
                let (child, changed) = self.Optimize(*child)?;
                Ok((
                    LogicalPlan::Projection {
                        expressions,
                        child: Box::new(child),
                        schema,
                    },
                    changed,
                ))
            }
            LogicalPlan::Join {
                join_type,
                left,
                right,
                equal_conditions,
                other_conditions,
                schema,
            } => {
                let (left, _) = self.Optimize(*left)?;
                let (right, changed) = self.Optimize(*right)?;
                Ok((
                    LogicalPlan::Join {
                        join_type,
                        left: Box::new(left),
                        right: Box::new(right),
                        equal_conditions,
                        other_conditions,
                        schema,
                    },
                    changed,
                ))
            }
            LogicalPlan::UnionAll { children, schema } => {
                let mut out = Vec::new();
                let mut changed = false;
                for child in children {
                    let (child, c) = self.Optimize(child)?;
                    changed = c;
                    out.push(child);
                }
                Ok((
                    LogicalPlan::UnionAll {
                        children: out,
                        schema,
                    },
                    changed,
                ))
            }
            LogicalPlan::Expand {
                child,
                grouping_sets,
                level_projections,
                schema,
            } => {
                let (child, changed) = self.Optimize(*child)?;
                Ok((
                    LogicalPlan::Expand {
                        child: Box::new(child),
                        grouping_sets,
                        level_projections,
                        schema,
                    },
                    changed,
                ))
            }
            other => Ok((other, false)),
        }
    }
    /// 规则注册名。
    pub fn Name(&self) -> &'static str {
        "skew_distinct_agg_rewrite"
    }
}
/// 分配合成聚合中间列表达式。
fn synthetic_column(next: &mut usize, field_type: &crate::task::FieldType) -> Expression {
    let column = *next;
    *next += 1;
    Expression {
        name: format!("agg_{column}"),
        column: Some(column),
        return_type: Some(field_type.clone()),
        ..Expression::default()
    }
}
