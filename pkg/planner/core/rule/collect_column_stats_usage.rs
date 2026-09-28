// Copyright 2026 AsterSQL.
/*
// Copyright 2021 PingCAP, Inc.
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

// 遍历逻辑计划收集列统计信息需求。
// 和 model 类型均是后续模块接线时使用的外部依赖。

// columnStatsUsageCollector 对应 Go 收集器：predicateCols 记录是否需要完整统计信息。
pub struct ColumnStatsUsageCollector {
    predicate_cols: Map<model::TableItemID, bool>,
    col_map: Map<i64, Set<model::TableItemID>>,
    cols: Vec<expression::Column>,
    visited_logical_tbl_ids: intset::FastIntSet,
    tbl_id_to_partition_ids: Map<i64, Vec<i64>>,
    operator_num: u64,
    interesting_cols_by_ds: Option<Map<logicalop::DataSource, Vec<expression::Column>>>,
    col_set: Set<i64>,
}

impl ColumnStatsUsageCollector {
    // newColumnStatsUsageCollector 预分配列缓存；index pruning 开关决定是否建立额外集合。
    fn new(collect_index_pruning_cols: bool) -> Self {
        Self {
            predicate_cols: Map::new(), col_map: Map::new(), cols: Vec::with_capacity(8),
            visited_logical_tbl_ids: intset::FastIntSet::new(),
            tbl_id_to_partition_ids: Map::new(), operator_num: 0,
            interesting_cols_by_ds: collect_index_pruning_cols.then(Map::new),
            col_set: Set::new(),
        }
    }

    // addPredicateColumn 把表达式列映射到真实表列；完整统计请求优先级高于 meta 请求。
    fn add_predicate_column(&mut self, col: &expression::Column, need_full_stats: bool) {
        let Some(tbl_col_ids) = self.col_map.get(&col.unique_id) else { return; };
        for id in tbl_col_ids {
            let old = self.predicate_cols.get(id).copied();
            if old == Some(true) || (old == Some(false) && !need_full_stats) { continue; }
            self.predicate_cols.insert(id.clone(), need_full_stats);
        }
    }

    // 从表达式提取普通列和相关列，再统一登记统计需求。
    fn add_predicate_columns_from_expressions(&mut self, list: &[expression::Expression], full: bool) {
        self.cols = expression::extract_columns_and_cor_columns(list);
        for col in &self.cols.clone() { self.add_predicate_column(col, full); }
    }

    // updateColMap 传播 Union/Projection 等算子输出列对应的底层表列集合。
    fn update_col_map(&mut self, col: &expression::Column, related: &[expression::Column]) {
        let ids = self.col_map.entry(col.unique_id).or_default();
        for related_col in related {
            if let Some(source) = self.col_map.get(&related_col.unique_id) { ids.extend(source.iter().cloned()); }
        }
    }

    fn update_col_map_from_expressions(&mut self, col: &expression::Column, list: &[expression::Expression]) {
        self.cols = expression::extract_columns_and_cor_columns(list);
        self.update_col_map(col, &self.cols.clone());
    }

    // DataSource 是统计收集的叶节点；系统表和内部伪列必须跳过。
    fn collect_predicate_columns_for_data_source(&mut self, asked: &[Vec<expression::Column>], ds: &mut logicalop::DataSource) {
        if filter::is_system_schema(&ds.db_name.to_lowercase()) {
            intest::assert_true(!ds.sctx().session_vars().in_restricted_sql(), "system table should have been skipped");
            return;
        }
        let table_id = ds.table_info.id;
        self.visited_logical_tbl_ids.insert(table_id as i32);
        if table_id != ds.physical_table_id { self.tbl_id_to_partition_ids.entry(table_id).or_default().push(ds.physical_table_id); }
        for col in &ds.schema().columns {
            if col.id <= 0 { continue; } // _tidb_rowid 等伪列没有持久化统计信息。
            self.col_map.insert(col.unique_id, Set::from([model::TableItemID { table_id, id: col.id, is_index: false }]));
        }
        for group in asked {
            if group.iter().all(|col| ds.schema().contains(col)) { ds.asked_column_group.push(group.clone()); }
        }
        // 这里使用 PushedDownConds；AllConds 仅服务分区裁剪，不需要统计信息。
        self.add_predicate_columns_from_expressions(&ds.pushed_down_conds, true);
    }

    // Join 的等值、左右条件和其它条件只需要 meta 级别的列统计信息。
    fn collect_predicate_columns_for_join(&mut self, p: &logicalop::LogicalJoin) {
        let mut exprs = Vec::new();
        exprs.extend(p.equal_conditions.iter().cloned()); exprs.extend(p.left_conditions.iter().cloned());
        exprs.extend(p.right_conditions.iter().cloned()); exprs.extend(p.other_conditions.iter().cloned());
        self.add_predicate_columns_from_expressions(&exprs, false);
    }

    // UnionAll 第 i 个输出列的统计信息来自每个子计划的第 i 列。
    fn collect_predicate_columns_for_union_all(&mut self, p: &logicalop::LogicalUnionAll) {
        for (i, col) in p.schema().columns.iter().enumerate() {
            let related: Vec<_> = p.children().iter().map(|child| child.schema().columns[i].clone()).collect();
            self.update_col_map(col, &related);
        }
    }

    // 收集 WHERE、JOIN、ORDER BY/GROUP BY 中会影响索引裁剪的列，并在同一次遍历中去重。
    fn collect_interesting_columns_for_data_source(&mut self, ds: &logicalop::DataSource, joins: &[expression::Column], ordering: &[expression::Column]) {
        self.col_set.clear();
        let mut all = Vec::new();
        let mut conditions = ds.pushed_down_conds.clone(); conditions.extend(ds.all_conds.clone());
        for cond in conditions { let cols = expression::extract_columns(&cond); if cols.iter().all(|c| ds.schema().contains(c)) { for col in cols { if self.col_set.insert(col.unique_id) { all.push(col); } } } }
        for col in joins.iter().chain(ordering) { if ds.schema().contains(col) && self.col_set.insert(col.unique_id) { all.push(col.clone()); } }
        if let Some(map) = &mut self.interesting_cols_by_ds { map.insert(ds.clone(), all); }
    }

    // collectFromPlan 先递归子节点，再按节点类型建立列映射和统计需求。
    // join/order 列沿树向下传递，供叶子 DataSource 的索引裁剪使用。
    fn collect_from_plan(&mut self, asked: Option<Vec<Vec<expression::Column>>>, lp: &mut dyn base::LogicalPlan, mut joins: Vec<expression::Column>, mut ordering: Vec<expression::Column>) {
        let current_groups = lp.extract_col_groups(asked.clone());
        if self.interesting_cols_by_ds.is_some() {
            match lp.kind() {
                base::LogicalKind::Join | base::LogicalKind::Apply => joins.extend(lp.join_columns()),
                base::LogicalKind::Sort | base::LogicalKind::TopN => ordering.extend(lp.ordering_columns()),
                base::LogicalKind::Window => ordering.extend(lp.window_partition_columns()),
                base::LogicalKind::Aggregation => { ordering.extend(lp.group_by_columns()); ordering.extend(lp.min_max_columns()); }
                _ => {}
            }
        }
        for child in lp.children_mut() { self.collect_from_plan(Some(current_groups.clone()), child, joins.clone(), ordering.clone()); }
        match lp.kind() {
            base::LogicalKind::DataSource => { let ds = lp.as_data_source_mut(); self.collect_predicate_columns_for_data_source(asked.as_deref().unwrap_or(&[]), ds); if self.interesting_cols_by_ds.is_some() { self.collect_interesting_columns_for_data_source(ds, &joins, &ordering); } }
            base::LogicalKind::IndexScan | base::LogicalKind::TableScan => { let (ds, conds) = lp.scan_source_and_conditions(); self.collect_predicate_columns_for_data_source(asked.as_deref().unwrap_or(&[]), ds); self.add_predicate_columns_from_expressions(conds, true); }
            base::LogicalKind::Projection => { for (col, expr) in lp.schema().columns.iter().zip(lp.expressions()) { self.update_col_map_from_expressions(col, std::slice::from_ref(expr)); } }
            base::LogicalKind::Selection => self.add_predicate_columns_from_expressions(lp.conditions(), false),
            base::LogicalKind::Aggregation => { self.add_predicate_columns_from_expressions(lp.group_by_items(), false); for (col, args) in lp.schema().columns.iter().zip(lp.aggregate_args()) { self.update_col_map_from_expressions(col, args); } }
            base::LogicalKind::Window => { for col in lp.window_partition_columns() { self.add_predicate_column(col, false); } for (col, args) in lp.window_result_and_args() { self.update_col_map_from_expressions(col, args); } }
            base::LogicalKind::Join => self.collect_predicate_columns_for_join(lp.as_join()),
            base::LogicalKind::Apply => { self.collect_predicate_columns_for_join(lp.as_apply().join()); for col in lp.correlated_columns() { self.add_predicate_column(col, false); } }
            base::LogicalKind::Sort | base::LogicalKind::TopN => { for expr in lp.ordering_expressions() { self.add_predicate_columns_from_expressions(std::slice::from_ref(expr), false); } }
            base::LogicalKind::UnionAll | base::LogicalKind::PartitionUnionAll => self.collect_predicate_columns_for_union_all(lp.as_union_all()),
            base::LogicalKind::CTE => { for plan in lp.cte_parts() { self.collect_from_plan(None, plan, Vec::new(), Vec::new()); } for (col, related) in lp.cte_related_columns() { self.update_col_map(col, related); } if lp.cte_is_distinct() { for col in &lp.schema().columns { self.add_predicate_column(col, false); } } }
            base::LogicalKind::CTETable => { for (col, seed) in lp.schema().columns.iter().zip(lp.seed_schema_columns()) { self.update_col_map(col, std::slice::from_ref(seed)); } }
            _ => {}
        }
        self.operator_num += 1;
    }
}

// CollectColumnStatsUsage 返回谓词列、逻辑表 ID、静态分区 ID 和算子数量。
// 计划捕获与 index pruning 的写回位置均保留，真实副作用由后续 Rust 模块接管。
pub fn collect_column_stats_usage(lp: &mut dyn base::LogicalPlan) -> (Map<model::TableItemID, bool>, intset::FastIntSet, Map<i64, Vec<i64>>, u64) {
    let threshold = lp.sctx().session_vars().opt_index_prune_threshold();
    let collect_index_pruning_cols = threshold >= 0;
    let plan_capture = lp.sctx().session_vars().is_plan_replayer_capture_enabled();
    let mut collector = ColumnStatsUsageCollector::new(collect_index_pruning_cols);
    collector.collect_from_plan(None, lp, Vec::new(), Vec::new());
    if plan_capture {
        let visited = collector.visited_logical_tbl_ids.to_i64_set();
        record_table_runtime_stats(lp.sctx(), visited);
    }
    if let Some(map) = collector.interesting_cols_by_ds.take() { for (ds, cols) in map { ds.set_interesting_columns(cols); } }
    (collector.predicate_cols, collector.visited_logical_tbl_ids, collector.tbl_id_to_partition_ids, collector.operator_num)
}
*/

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
