// Copyright 2026 AsterSQL.

// 遍历逻辑计划（Logical Plan）收集列统计信息使用情况。
//
// 统计信息（Statistics）用于代价估算：谓词列标记是否需要完整直方图，
// 并记录访问表、静态分区及对索引裁剪（index pruning）有用的列。

use crate::rule_init::{AggKind, Expr, Plan, PlanKind};
use std::collections::{BTreeMap, BTreeSet};

/// 一次遍历后汇总的列统计使用结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnStatsUsage {
    /// 谓词涉及的列 ID → 是否需要完整统计（true=full，false=仅 meta）。
    pub predicate_columns: BTreeMap<i64, bool>,
    /// 遍历中访问过的逻辑表 ID 集合。
    pub visited_tables: BTreeSet<i64>,
    /// 逻辑表 ID → 选中的物理分区 ID 集合（静态分区裁剪结果）。
    pub table_partitions: BTreeMap<i64, BTreeSet<i64>>,
    /// 遍历到的算子数量。
    pub operator_count: usize,
    /// 表 ID → 对索引裁剪有意义的列集合（join/order 等下推列）。
    pub interesting_columns: BTreeMap<i64, BTreeSet<i64>>,
}

/// 从逻辑计划根节点收集列统计使用情况。
///
/// `collect_index_pruning_columns` 为 true 时，还会收集索引裁剪所需的列。
pub fn collect_column_stats_usage(
    plan: &Plan,
    collect_index_pruning_columns: bool,
) -> ColumnStatsUsage {
    let mut usage = ColumnStatsUsage::default();
    collect(plan, &mut usage, &[], &[], collect_index_pruning_columns);
    usage
}

/// 递归遍历计划树：登记谓词列，并按算子类型下传 join/order 列。
fn collect(
    plan: &Plan,
    usage: &mut ColumnStatsUsage,
    join_columns: &[i64],
    ordering_columns: &[i64],
    collect_indexes: bool,
) {
    usage.operator_count += 1;
    match &plan.kind {
        PlanKind::DataSource {
            table_id,
            indexes: _,
            partition,
            selected_partitions,
        } => {
            usage.visited_tables.insert(*table_id);
            // 将选中的分区定义下标映射为物理分区 ID。
            if let (Some(partition), Some(selected)) = (partition, selected_partitions) {
                let partitions: BTreeSet<i64> = selected
                    .iter()
                    .filter_map(|index: &usize| {
                        partition
                            .definitions
                            .get(*index)
                            .map(|definition| definition.id)
                    })
                    .collect();
                usage
                    .table_partitions
                    .entry(*table_id)
                    .or_default()
                    .extend(partitions);
            }
            if collect_indexes {
                // Go 的开关只控制 WHERE/JOIN/ORDER/GROUP 对索引裁剪的提示，
                // 不会仅因索引存在就把其首列登记为谓词统计需求。
                let interesting = usage.interesting_columns.entry(*table_id).or_default();
                interesting.extend(
                    join_columns
                        .iter()
                        .chain(ordering_columns)
                        .filter(|column| plan.schema.contains(column))
                        .copied(),
                );
                for predicate in &plan.predicates {
                    let columns = predicate.columns();
                    if columns.iter().all(|column| plan.schema.contains(column)) {
                        interesting.extend(columns);
                    }
                }
            }
            // Conditions attached to a data source are pushed-down predicates;
            // their histograms may be needed in full.
            for predicate in &plan.predicates {
                add_expression(usage, predicate, true);
            }
        }
        PlanKind::Join {
            equal_conditions,
            other_conditions,
            ..
        } => {
            // Join 条件列向下传给子 DataSource，作为 interesting columns。
            let join = equal_conditions
                .iter()
                .chain(other_conditions)
                .flat_map(Expr::columns)
                .collect::<Vec<_>>();
            for condition in equal_conditions.iter().chain(other_conditions) {
                // Join cardinality currently needs only column metadata (for
                // example NDV), matching Go's join collector.
                add_expression(usage, condition, false);
            }
            for child in &plan.children {
                collect(child, usage, &join, ordering_columns, collect_indexes);
            }
            return;
        }
        PlanKind::Sort { by } => {
            // ORDER BY 列向下传，供叶子索引裁剪。
            let ordering = by.iter().flat_map(Expr::columns).collect::<Vec<_>>();
            for expression in by {
                add_expression(usage, expression, false);
            }
            for child in &plan.children {
                collect(child, usage, join_columns, &ordering, collect_indexes);
            }
            return;
        }
        PlanKind::Aggregation {
            aggregates,
            group_by,
        } => {
            for expression in group_by {
                add_expression(usage, expression, false);
            }
            // Go 通过聚合输出列血缘按需传播普通聚合参数，而不是直接把
            // SUM/COUNT 等参数登记为谓词列。精简 IR 尚无父级血缘表；仅保留
            // DISTINCT 的 GroupNDV 输入契约。
            for aggregate in aggregates {
                if aggregate.distinct {
                    for expression in &aggregate.args {
                        add_expression(usage, expression, false);
                    }
                }
            }
            if collect_indexes {
                let mut ordering = group_by.iter().flat_map(Expr::columns).collect::<Vec<_>>();
                ordering.extend(
                    aggregates
                        .iter()
                        .filter(|aggregate| matches!(aggregate.kind, AggKind::Min | AggKind::Max))
                        .flat_map(|aggregate| aggregate.args.iter().flat_map(Expr::columns)),
                );
                for child in &plan.children {
                    collect(child, usage, join_columns, &ordering, collect_indexes);
                }
                return;
            }
        }
        PlanKind::Selection => {
            // Selection and ordering statistics are metadata-only in Go.
            for predicate in &plan.predicates {
                add_expression(usage, predicate, false);
            }
        }
        PlanKind::Projection { .. }
        | PlanKind::Limit { .. }
        | PlanKind::UnionAll
        | PlanKind::PartitionUnion
        | PlanKind::TableDual { .. }
        | PlanKind::Other => {
            for predicate in &plan.predicates {
                add_expression(usage, predicate, false);
            }
        }
    }
    for child in &plan.children {
        collect(
            child,
            usage,
            join_columns,
            ordering_columns,
            collect_indexes,
        );
    }
}
/// 将表达式中的列登记到 predicate_columns；full 与已有值按位或合并。
fn add_expression(usage: &mut ColumnStatsUsage, expression: &Expr, full: bool) {
    for column in expression.columns() {
        usage
            .predicate_columns
            .entry(column)
            .and_modify(|value| *value |= full)
            .or_insert(full);
    }
}
