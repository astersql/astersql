// Copyright 2017 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
// Copyright 2026 AsterSQL.

// 聚合消除（Aggregation Elimination）逻辑优化规则。
//
// 当分组键已是子计划唯一键或最多一行时，把聚合改写成投影；
// 若 DISTINCT 参数已覆盖唯一键，则去掉 DISTINCT。聚合指 COUNT/SUM 等分组汇总算子。

use crate::task::{Expression, FieldType, JoinType, PlanNode};
use std::collections::HashSet;

/// 本模块结果类型：错误以 String 描述。
pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 聚合函数名称枚举（对应 Go aggregation.AggFuncDesc.Name）。
pub enum AggFuncName {
    Count,
    Sum,
    Avg,
    FirstRow,
    Max,
    Min,
    GroupConcat,
    BitAnd,
    BitOr,
    BitXor,
    ApproxCountDistinct,
    JsonArrayAgg,
    JsonObjectAgg,
    Other(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 聚合执行模式：Complete/Partial/Final/Dedup 等两阶段拆分阶段。
pub enum AggMode {
    Complete,
    Partial1,
    Partial2,
    Final,
    Dedup,
}
#[derive(Clone, Debug)]
/// 单个聚合函数描述：名称、参数、是否 DISTINCT、模式与返回类型。
pub struct AggFuncDesc {
    pub name: AggFuncName,
    pub args: Vec<Expression>,
    pub distinct: bool,
    pub mode: AggMode,
    pub return_type: FieldType,
    pub order_by: Vec<Expression>,
}
#[derive(Clone, Debug)]
/// 逻辑聚合算子：聚合函数列表、分组项与子计划。
pub struct LogicalAggregation {
    pub agg_funcs: Vec<AggFuncDesc>,
    pub group_by_items: Vec<Expression>,
    pub child: Box<LogicalPlan>,
    pub schema: Vec<FieldType>,
    pub output_columns: Vec<usize>,
    pub no_eliminate: bool,
}
#[derive(Clone, Debug)]
/// 本规则使用的简化逻辑计划枚举（聚合/投影/连接/并集/Expand/叶子）。
pub enum LogicalPlan {
    Aggregation(LogicalAggregation),
    Projection {
        expressions: Vec<Expression>,
        child: Box<LogicalPlan>,
        schema: Vec<FieldType>,
    },
    Join {
        join_type: JoinType,
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        equal_conditions: Vec<(usize, usize)>,
        other_conditions: Vec<Expression>,
        schema: Vec<FieldType>,
    },
    UnionAll {
        children: Vec<LogicalPlan>,
        schema: Vec<FieldType>,
    },
    Expand {
        child: Box<LogicalPlan>,
        grouping_sets: Vec<Vec<usize>>,
        level_projections: Vec<Vec<Expression>>,
        schema: Vec<FieldType>,
    },
    Node {
        node: PlanNode,
        unique_keys: Vec<Vec<usize>>,
        max_one_row: bool,
    },
}
impl LogicalPlan {
    /// 返回该计划节点输出 schema（字段类型列表）。
    pub fn schema(&self) -> &[FieldType] {
        match self {
            Self::Aggregation(p) => &p.schema,
            Self::Projection { schema, .. }
            | Self::Join { schema, .. }
            | Self::UnionAll { schema, .. }
            | Self::Expand { schema, .. } => schema,
            Self::Node { node, .. } => &node.schema,
        }
    }
    /// 返回唯一键列集合；投影透传子节点，其它默认空。
    pub fn unique_keys(&self) -> Vec<Vec<usize>> {
        match self {
            Self::Node { unique_keys, .. } => unique_keys.clone(),
            Self::Projection { child, .. } => child.unique_keys(),
            _ => Vec::new(),
        }
    }
    /// 是否保证最多输出一行（Max One Row）。
    pub fn max_one_row(&self) -> bool {
        matches!(
            self,
            Self::Node {
                max_one_row: true,
                ..
            }
        )
    }
}

#[derive(Default)]
/// 聚合消除规则入口。
pub struct AggregationEliminator;
/// 聚合消除检查器：尝试改写为投影或去掉 DISTINCT。
#[derive(Default)]
pub struct aggregationEliminateChecker {
    /// 聚合下推后旧聚合需要额外检查外连接可空侧。
    pub old_agg_elimination_check: bool,
}

impl aggregationEliminateChecker {
    /// 若可把聚合转为投影则返回新计划，否则 None。
    pub fn tryToEliminateAggregation(&self, agg: &LogicalAggregation) -> Option<LogicalPlan> {
        if agg.no_eliminate
            || agg
                .agg_funcs
                .iter()
                .any(|function| function.name == AggFuncName::GroupConcat)
        {
            return None;
        }
        let grouped: HashSet<_> = agg.group_by_items.iter().filter_map(|e| e.column).collect();
        let covered_by_unique_key = agg
            .child
            .unique_keys()
            .iter()
            .any(|key| !key.is_empty() && key.iter().all(|column| grouped.contains(column)));
        if !covered_by_unique_key
            || (self.old_agg_elimination_check && !CheckCanConvertAggToProj(agg))
        {
            return None;
        }
        ConvertAggToProj(agg, agg.child.schema()).1
    }
    /// 若 DISTINCT 参数已覆盖子计划唯一键，则清除 distinct 标志。
    pub fn tryToEliminateDistinct(&self, agg: &mut LogicalAggregation) {
        let keys = agg.child.unique_keys();
        for function in &mut agg.agg_funcs {
            if !function.distinct {
                continue;
            }
            // DISTINCT 参数列集合若覆盖任一唯一键，则 DISTINCT 无意义。
            let Some(columns) = function
                .args
                .iter()
                .map(|expression| expression.column)
                .collect::<Option<HashSet<_>>>()
            else {
                continue;
            };
            if keys
                .iter()
                .any(|key| key.iter().all(|column| columns.contains(column)))
            {
                function.distinct = false;
            }
        }
    }
    /// Semi Join 内侧 DISTINCT（仅 FirstRow）是否可因唯一键而消除。
    pub fn canEliminateSemiJoinInnerDistinct(&self, agg: &LogicalAggregation) -> bool {
        if agg.group_by_items.is_empty()
            || has_limit(&agg.child)
            || agg.agg_funcs.iter().any(|function| {
                function.name != AggFuncName::FirstRow
                    || function.distinct
                    || !function.order_by.is_empty()
                    || function.args.len() != 1
            })
        {
            return false;
        }
        true
    }
}

/// 检查聚合下推后的旧聚合能否在外连接上安全投影化。
pub fn CheckCanConvertAggToProj(agg: &LogicalAggregation) -> bool {
    let LogicalPlan::Join {
        join_type,
        left,
        right,
        ..
    } = agg.child.as_ref()
    else {
        return true;
    };
    let may_null_range = match join_type {
        JoinType::LeftOuter => left.schema().len()..left.schema().len() + right.schema().len(),
        JoinType::RightOuter => 0..left.schema().len(),
        _ => return true,
    };
    !agg.agg_funcs
        .iter()
        .flat_map(|function| &function.args)
        .any(|arg| {
            arg.column
                .is_some_and(|column| may_null_range.contains(&column))
        })
}
/// 把合格聚合改写成投影；失败返回 (false, None)。
pub fn ConvertAggToProj(
    agg: &LogicalAggregation,
    schema: &[FieldType],
) -> (bool, Option<LogicalPlan>) {
    let mut expressions = Vec::new();
    for function in &agg.agg_funcs {
        let Some(expr) = rewriteExpr(function) else {
            return (false, None);
        };
        expressions.push(expr);
    }
    (
        true,
        Some(LogicalPlan::Projection {
            expressions,
            child: agg.child.clone(),
            schema: if agg.schema.is_empty() {
                schema.to_vec()
            } else {
                agg.schema.clone()
            },
        }),
    )
}
/// 将单个聚合函数改写成无聚合标量表达式（按函数种类分支）。
pub fn rewriteExpr(function: &AggFuncDesc) -> Option<Expression> {
    let arg = function
        .args
        .first()
        .cloned()
        .unwrap_or_else(|| Expression {
            name: "1".into(),
            ..Expression::default()
        });
    match function.name {
        AggFuncName::Count => Some(rewriteCount(&function.args, &function.return_type)),
        AggFuncName::BitAnd | AggFuncName::BitOr | AggFuncName::BitXor => {
            Some(rewriteBitFunc(&function.name, &arg, &function.return_type))
        }
        AggFuncName::Sum
        | AggFuncName::Avg
        | AggFuncName::FirstRow
        | AggFuncName::Max
        | AggFuncName::Min
        | AggFuncName::GroupConcat => Some(wrapCastFunction(&arg, &function.return_type)),
        AggFuncName::ApproxCountDistinct
        | AggFuncName::JsonArrayAgg
        | AggFuncName::JsonObjectAgg
        | AggFuncName::Other(_) => None,
    }
}
/// COUNT 改写：无参为 1；有参则用 isnull 判断后输出 0/1。
pub fn rewriteCount(exprs: &[Expression], targetTp: &FieldType) -> Expression {
    let name = if exprs.is_empty() {
        "1".into()
    } else {
        let null_checks = exprs
            .iter()
            .map(|e| format!("isnull({})", e.name))
            .collect::<Vec<_>>()
            .join(" or ");
        format!("if({null_checks},0,1)")
    };
    Expression {
        name,
        return_type: Some(targetTp.clone()),
        ..Expression::default()
    }
}
/// 位聚合改写：空值用恒等元（BitAnd 全 1，其余 0）填充后 cast。
pub fn rewriteBitFunc(kind: &AggFuncName, arg: &Expression, targetTp: &FieldType) -> Expression {
    let identity = match kind {
        AggFuncName::BitAnd => "18446744073709551615",
        _ => "0",
    };
    Expression {
        name: format!("ifnull(cast({}),{identity})", arg.name),
        return_type: Some(targetTp.clone()),
        ..Expression::default()
    }
}
/// 为 SUM/AVG 等包一层 cast 到目标返回类型。
pub fn wrapCastFunction(arg: &Expression, targetTp: &FieldType) -> Expression {
    if arg.return_type.as_ref() == Some(targetTp) {
        return arg.clone();
    }
    Expression {
        name: format!("cast({})", arg.name),
        column: arg.column,
        return_type: Some(targetTp.clone()),
        ..Expression::default()
    }
}

/// AggregationEliminator 的 LogicalOptRule 风格接口。
impl AggregationEliminator {
    /// 先优化子树，再尝试去掉 DISTINCT / 把聚合改成投影。
    pub fn Optimize(&self, plan: LogicalPlan) -> Result<(LogicalPlan, bool)> {
        let mut changed = false;
        let mut plan = optimize_children(self, plan, &mut changed)?;
        if let LogicalPlan::Join {
            join_type, right, ..
        } = &mut plan
            && matches!(join_type, JoinType::Semi | JoinType::AntiSemi)
            && let LogicalPlan::Aggregation(aggregation) = right.as_ref()
            && aggregationEliminateChecker::default().canEliminateSemiJoinInnerDistinct(aggregation)
        {
            *right = aggregation.child.clone();
            return Ok((plan, true));
        }
        if let LogicalPlan::Aggregation(mut agg) = plan {
            let checker = aggregationEliminateChecker::default();
            checker.tryToEliminateDistinct(&mut agg);
            if let Some(projection) = checker.tryToEliminateAggregation(&agg) {
                return Ok((projection, changed));
            }
            Ok((LogicalPlan::Aggregation(agg), changed))
        } else {
            Ok((plan, changed))
        }
    }
    /// 规则注册名。
    pub fn Name(&self) -> &'static str {
        "aggregation_eliminate"
    }
}

fn has_limit(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Node { node, .. } => node.kind == crate::task::PlanKind::Limit,
        LogicalPlan::Aggregation(aggregation) => has_limit(&aggregation.child),
        LogicalPlan::Projection { child, .. } | LogicalPlan::Expand { child, .. } => {
            has_limit(child)
        }
        LogicalPlan::Join { left, right, .. } => has_limit(left) || has_limit(right),
        LogicalPlan::UnionAll { children, .. } => children.iter().any(has_limit),
    }
}
/// 递归优化各算子孩子，汇总 changed 标志。
fn optimize_children(
    rule: &AggregationEliminator,
    plan: LogicalPlan,
    changed: &mut bool,
) -> Result<LogicalPlan> {
    Ok(match plan {
        LogicalPlan::Aggregation(mut agg) => {
            let (child, c) = rule.Optimize(*agg.child)?;
            *changed |= c;
            agg.child = Box::new(child);
            LogicalPlan::Aggregation(agg)
        }
        LogicalPlan::Projection {
            expressions,
            child,
            schema,
        } => {
            let (child, c) = rule.Optimize(*child)?;
            *changed |= c;
            LogicalPlan::Projection {
                expressions,
                child: Box::new(child),
                schema,
            }
        }
        LogicalPlan::Join {
            join_type,
            left,
            right,
            equal_conditions,
            other_conditions,
            schema,
        } => {
            let (left, l) = rule.Optimize(*left)?;
            let (right, r) = rule.Optimize(*right)?;
            *changed |= l || r;
            LogicalPlan::Join {
                join_type,
                left: Box::new(left),
                right: Box::new(right),
                equal_conditions,
                other_conditions,
                schema,
            }
        }
        LogicalPlan::UnionAll { children, schema } => {
            let mut out = Vec::new();
            for child in children {
                let (child, c) = rule.Optimize(child)?;
                *changed |= c;
                out.push(child);
            }
            LogicalPlan::UnionAll {
                children: out,
                schema,
            }
        }
        LogicalPlan::Expand {
            child,
            grouping_sets,
            level_projections,
            schema,
        } => {
            let (child, c) = rule.Optimize(*child)?;
            *changed |= c;
            LogicalPlan::Expand {
                child: Box::new(child),
                grouping_sets,
                level_projections,
                schema,
            }
        }
        node => node,
    })
}
