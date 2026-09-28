// Copyright 2026 AsterSQL.
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

// 优化阶段收集列/索引统计信息的流程。
// domain、infoschema、model、logicalop、statistics 等 Go 包在此保留为外部 stub 依赖。
//
// CollectPredicateColumnsPoint 收集谓词使用的列，并准备后续统计信息加载请求。
// pub struct CollectPredicateColumnsPoint;
// const SKIP_PLAN_CACHE_REASON_SYNC_LOAD_FALLBACK: &str = "sync-load timed out and fell back to pseudo stats";
//
// impl CollectPredicateColumnsPoint {
// Optimize 保留 Go 的主流程：收集使用情况、预取表元数据、裁剪索引、
// 补齐虚拟列和索引，最后按同步/异步模式提交统计加载请求。
//     pub fn Optimize(&self, plan: base::LogicalPlan) -> (base::LogicalPlan, bool, Error) {
//         let changed = false;
//         intest::assert_condition(!plan.sctx().vars().in_restricted_sql
//             || (plan.sctx().vars().internal_sql_scan_user_table
//                 && plan.sctx().vars().in_restricted_sql),
//             "CollectPredicateColumnsPoint should not be called in restricted SQL mode");
//         let sync_wait = plan.sctx().vars().stats_load_sync_wait.load();
//         let sync_enabled = sync_wait > 0;
//         let (predicate_columns, visited_ids, partition_ids, op_num) = CollectColumnStatsUsage(&plan);
//         plan.sctx().vars().stmt_ctx.operator_num = op_num;
//         if !predicate_columns.is_empty() { plan.sctx().update_col_stats_usage(predicate_columns.keys()); }
//
// 先缓存 TableInfo，避免下面每个统计分支重复访问 InfoSchema。
//         let info_schema = plan.sctx().latest_info_schema();
//         let mut tables = Map::new();
//         visited_ids.for_each(|id| {
//             if let Some(table) = info_schema.table_info_by_id(id as i64) { tables.insert(id as i64, table); }
//         });
//         self.mark_at_least_one_full_stats_load_for_each_table(
//             plan.sctx(), &visited_ids, &mut tables, &mut predicate_columns.clone(), sync_enabled);
//         let needed_columns = predicate_columns.iter().map(|(item, full)|
//             StatsLoadItem::new(*item, *full)).collect::<Vec<_>>();
//         let kept = self.pruneIndexesForAllDataSources(&plan);
//         let virtual_columns = CollectDependingVirtualCols(&tables, &needed_columns);
//         let indices = collectSyncIndices(plan.sctx(), &append(needed_columns.clone(), virtual_columns), &tables, &kept);
//         let mut needed = collectHistNeededItems(&needed_columns, &indices);
//         needed = self.expandStatsNeededColumnsForStaticPruning(needed, &partition_ids);
//         if needed.is_empty() { return (plan, changed, Error::none()); }
//         if sync_enabled { return (plan, changed, RequestLoadStats(plan.sctx(), &needed, sync_wait)); }
// 静态裁剪发生在此规则之后，因此异步模式可能暂时多加载一些项目；这是 Go 的既有取舍。
//         for item in needed { asyncload::ASYNC_LOAD_HISTOGRAM_NEEDED_ITEMS.insert(item.id(), item.full_load()); }
//         (plan, changed, Error::none())
//     }
//
// 至少为每张访问表挑选一个尚未 full-load 的公开列/索引，保证确定性模式有触发点。
//     fn mark_at_least_one_full_stats_load_for_each_table(&self, ctx: planctx::PlanContext,
//         visited: &intset::FastIntSet, tables: &mut Map<i64, model::TableInfo>,
//         predicate: &mut Map<model::TableItemID, bool>, hist_needed: bool) {
//         let Some(stats) = domain::get_domain(ctx).stats_handle() else { return; };
//         let mut tables_with_needed = intset::FastIntSet::new();
//         for (item, full) in predicate.iter() {
//             if !*full { continue; }
//             let Some(table) = tables.get(&item.table_id()) else { continue; };
//             let Some(table_stats) = stats.physical_table_stats(table.id(), table) else { continue; };
//             if table_stats.pseudo() || !table_stats.col_idx_existence().has_analyzed(item.id(), item.is_index()) { continue; }
//             tables_with_needed.insert(item.table_id() as i32);
//         }
//         visited.for_each(|physical_id| {
//             let Some(table) = tables.get(&(physical_id as i64)) else { return; };
//             if tables_with_needed.has(physical_id) { return; }
//             let Some(table_stats) = stats.physical_table_stats(table.id(), table) else { return; };
//             if table_stats.pseudo() { return; }
// 选择第一列作为触发点；若已有 full-load 列或索引，则无需重复触发。
//             let mut trigger = None;
//             for col in table.columns() {
//                 if !col.is_public() || (col.is_generated() && !col.generated_stored())
//                     || !table_stats.col_idx_existence().has_analyzed(col.id(), false) { continue; }
//                 if table_stats.col(col.id()).is_some_and(|s| s.is_full_load()) { trigger = None; break; }
//                 trigger = Some(model::TableItemID::column(physical_id as i64, col.id())); break;
//             }
//             let Some(item) = trigger else { return; };
//             for idx in table.indices() {
//                 if !idx.is_public() || idx.is_mv() { continue; }
//                 if table_stats.idx(idx.id()).is_some_and(|s| s.is_full_load()) { return; }
//             }
//             if hist_needed { predicate.insert(item, true); }
//             else { asyncload::ASYNC_LOAD_HISTOGRAM_NEEDED_ITEMS.insert(item, true); }
//         });
//     }
//
// 递归遍历计划树，找到 DataSource、IndexScan、TableScan 并裁剪其 access paths。
//     fn pruneIndexesForAllDataSources(&self, plan: &base::LogicalPlan) -> Map<i64, Set<i64>> {
//         let mut kept = Map::new(); self.collectAndPruneDataSources(plan, &mut kept); kept
//     }
//     fn collectAndPruneDataSources(&self, plan: &base::LogicalPlan, kept: &mut Map<i64, Set<i64>>) {
//         match plan.kind() {
//             logicalop::Kind::DataSource(ds) => prune_indexes_for_data_source(ds, kept),
//             logicalop::Kind::IndexScan(scan) => prune_indexes_for_data_source(scan.source(), kept),
//             logicalop::Kind::TableScan(scan) => prune_indexes_for_data_source(scan.source(), kept),
//             _ => {}
//         }
//         for child in plan.children() { self.collectAndPruneDataSources(child, kept); }
//     }
//
// 静态分区裁剪兼容逻辑：在原切片末尾追加各分区同列号的加载项目。
//     fn expandStatsNeededColumnsForStaticPruning(&self, mut items: Vec<model::StatsLoadItem>, partitions: &Map<i64, Vec<i64>>) -> Vec<model::StatsLoadItem> {
//         let original_len = items.len();
//         for index in 0..original_len {
//             let item = items[index];
//             if let Some(ids) = partitions.get(&item.table_id()) {
//                 for pid in ids { items.push(model::StatsLoadItem::new(model::TableItemID::new(*pid, item.id(), item.is_index()), item.full_load())); }
//             }
//         }
//         items
//     }
//     pub fn Name(&self) -> &'static str { "collect_predicate_columns_point" }
// }
//
// pruneIndexesForDataSource 保留阈值、表路径、common-handle 主索引以及 union 合并逻辑。
// fn prune_indexes_for_data_source(ds: &mut logicalop::DataSource, kept: &mut Map<i64, Set<i64>>) {
//     let mut threshold = ds.sctx().vars().opt_index_prune_threshold;
//     if threshold < 0 || ds.all_possible_access_paths().len() <= 1 { return; }
//     if threshold == 0 { threshold = ds.all_possible_access_paths().len() as i64; }
//     let pruned = prune_indexes_by_where_and_order(ds, ds.all_possible_access_paths(), ds.interesting_columns(), threshold);
//     let mut table_kept = Set::new();
//     for path in &pruned {
//         if (!path.is_table_path() && path.index().is_some()) || (path.is_table_path() && path.is_common_handle() && path.index().is_some()) {
//             table_kept.insert(path.index().unwrap().id());
//         }
//     }
//     if pruned.len() < ds.all_possible_access_paths().len() {
//         kept.entry(ds.physical_table_id()).or_default().extend(table_kept);
//         ds.set_all_possible_access_paths(pruned.clone());
// Go copy 了 PossibleAccessPaths，避免两个 slice 共享底层数组；Rust 这里同样 clone。
//         ds.set_possible_access_paths(pruned);
//     }
// }
//
// SyncWaitStatsLoadPoint 是同步等待统计加载的第二个优化规则。
// pub struct SyncWaitStatsLoadPoint;
// impl SyncWaitStatsLoadPoint {
//     pub fn Optimize(&self, plan: base::LogicalPlan) -> (base::LogicalPlan, bool, Error) {
//         let changed = false;
//         intest::assert_condition(!plan.sctx().vars().in_restricted_sql || plan.sctx().vars().internal_sql_scan_user_table,
//             "SyncWaitStatsLoadPoint should not be called in restricted SQL mode");
//         if plan.sctx().vars().stmt_ctx.is_sync_stats_failed { return (plan, changed, Error::none()); }
//         (plan, changed, SyncWaitStatsLoad(&plan))
//     }
//     pub fn Name(&self) -> &'static str { "sync_wait_stats_load_point" }
// }
//
// RequestLoadStats 将等待时间限制到 max execution time，再发送请求；超时可按配置回退 pseudo stats。
// pub fn RequestLoadStats(ctx: base::PlanContext, items: &[model::StatsLoadItem], mut sync_wait: i64) -> Error {
//     let max_time = ctx.vars().max_execution_time();
//     if max_time > 0 && max_time < sync_wait as u64 { sync_wait = max_time as i64; }
// Go failpoint 在这里验证测试注入的 1ms 等待；仅保留该检查位置。
//     let timeout = Duration::from_millis(sync_wait.max(0) as u64);
//     let stmt = ctx.vars().stmt_ctx;
//     match domain::get_domain(ctx).stats_handle().send_load_requests(stmt, items, timeout) {
//         Ok(()) => Error::none(),
//         Err(err) => {
//             stmt.is_sync_stats_failed = true;
//             if vardef::STATS_LOAD_PSEUDO_TIMEOUT.load() {
//                 stmt.set_skip_plan_cache(SKIP_PLAN_CACHE_REASON_SYNC_LOAD_FALLBACK);
//                 logutil::warn("RequestLoadStats failed", err); stmt.append_warning(err); Error::none()
//             } else { logutil::warn("RequestLoadStats failed", err); err }
//         }
//     }
// }
//
// SyncWaitStatsLoad 等待所有 NeededItems 完成，并记录等待耗时；错误处理与上函数一致。
// pub fn SyncWaitStatsLoad(plan: &base::LogicalPlan) -> Error {
//     let stmt = plan.sctx().vars().stmt_ctx;
//     if stmt.stats_load.needed_items.is_empty() { return Error::none(); }
//     let begin = Instant::now();
//     let result = domain::get_domain(plan.sctx()).stats_handle().sync_wait_stats_load(stmt);
//     plan.sctx().vars().duration_optimizer.stats_sync_wait = begin.elapsed();
//     match result {
//         Ok(()) => Error::none(),
//         Err(err) if vardef::STATS_LOAD_PSEUDO_TIMEOUT.load() => { stmt.is_sync_stats_failed = true; stmt.set_skip_plan_cache(SKIP_PLAN_CACHE_REASON_SYNC_LOAD_FALLBACK); stmt.append_warning(err); Error::none() }
//         Err(err) => { stmt.is_sync_stats_failed = true; err }
//     }
// }
//
// CollectDependingVirtualCols 只检查直接依赖：需要列命中虚拟列表达式时，追加 full-load 项。
// pub fn CollectDependingVirtualCols(tables: &Map<i64, model::TableInfo>, items: &[model::StatsLoadItem]) -> Vec<model::StatsLoadItem> {
//     let mut needed_by_table: Map<i64, Set<i64>> = Map::new();
//     for item in items { if !item.is_index() { needed_by_table.entry(item.table_id()).or_default().insert(item.id()); } }
//     let mut result = Vec::new();
//     for (table_id, ids) in needed_by_table {
//         let Some(table) = tables.get(&table_id) else { continue; };
//         let names = ids.iter().filter_map(|id| table.find_column_name_by_id(*id)).collect::<Set<_>>();
//         for col in table.columns() {
//             if !col.is_public() || !col.is_virtual_generated() || names.contains(&col.name()) { continue; }
//             if col.dependences().iter().any(|dep| names.contains(dep)) {
//                 result.push(model::StatsLoadItem::new(model::TableItemID::column(table_id, col.id()), true));
//             }
//         }
//     }
//     result
// }
//
// collectSyncIndices 只收集公开、未 full-load、且未被 access-path pruning 淘汰的索引。
// pub fn collectSyncIndices(ctx: base::PlanContext, columns: &[model::StatsLoadItem], tables: &Map<i64, model::TableInfo>, kept: &Map<i64, Set<i64>>) -> Set<model::TableItemID> {
//     let stats = domain::get_domain(ctx).stats_handle(); let mut result = Set::new();
//     for column in columns {
//         if column.is_index() { continue; }
//         let Some(table) = tables.get(&column.table_id()) else { continue; };
//         let Some(name) = table.find_column_name_by_id(column.id()) else { continue; };
//         for index in table.indices() {
//             if !index.is_public() || index.find_column_by_name(&name).is_none() { continue; }
//             if let Some(allowed) = kept.get(&column.table_id()) && !allowed.contains(&index.id()) { continue; }
//             let Some(ts) = stats.physical_table_stats(table.id(), table) else { continue; };
//             if ts.pseudo() || !ts.index_is_load_needed(index.id()) { continue; }
//             result.insert(model::TableItemID::index(column.table_id(), index.id()));
//         }
//     }
//     result
// }
//
// collectHistNeededItems 保持 Go 的列项目顺序，再追加索引项目并标记 full-load。
// pub fn collectHistNeededItems(columns: &[model::StatsLoadItem], indices: &Set<model::TableItemID>) -> Vec<model::StatsLoadItem> {
//     let mut result = columns.to_vec();
//     result.extend(indices.iter().map(|idx| model::StatsLoadItem::new(*idx, true))); result
// }
//
// recordTableRuntimeStats 为每张表记录 JSON 统计；单表失败只告警并继续其他表。
// pub fn recordTableRuntimeStats(ctx: base::PlanContext, tables: &Set<i64>) {
//     let mut recorded = ctx.vars().stmt_ctx.table_stats.take().unwrap_or_default();
//     for id in tables {
//         let (stats, skip, err) = recordSingleTableRuntimeStats(ctx, *id);
//         if err.is_some() { logutil::warn("record table json stats failed", err); }
//         if stats.is_none() && !skip { logutil::warn("record table json stats failed due to empty", id); }
//         recorded.insert(*id, stats);
//     }
//     ctx.vars().stmt_ctx.table_stats = Some(recorded);
// }
//
// recordSingleTableRuntimeStats 查询 InfoSchema 元数据并读取物理表统计；临时表跳过空统计告警。
// pub fn recordSingleTableRuntimeStats(ctx: base::PlanContext, table_id: i64) -> (Option<statistics::Table>, bool, Error) {
//     let domain = domain::get_domain(ctx); let schema = ctx.latest_info_schema();
//     let Some(table) = schema.table_by_id(table_id) else { return (None, false, Error::none()); };
//     let meta = table.meta(); let stats = domain.stats_handle().physical_table_stats(meta.id(), &meta);
//     let skip = meta.temp_table_type() != model::TempTableType::None;
//     (stats, skip, Error::none())
// }
//
// Go 来源导入：context、maps、time、failpoint、domain、infoschema、model、base、logicalop、planctx、util、vardef、statistics、asyncload、intest、intset、logutil、zap。
// Map/Set、Error、Duration、Instant 及 planner 类型均是迁移占位，未声明跨文件依赖，便于后续统一接线。
// */
// 优化阶段收集计划所需列/索引统计信息的规则与辅助函数。
//
// 对应 Go 的 CollectPredicateColumnsPoint：先汇总谓词列使用，
// 再触发同步/异步统计加载。下方注释块保留完整 Go 流程迁移草稿。

use crate::collect_column_stats_usage::collect_column_stats_usage;
use crate::rule_init::{LogicalRule, Plan, UsedStats};
use std::collections::{BTreeMap, BTreeSet};

/// 同步加载超时回退到伪统计（pseudo stats）时，跳过计划缓存的原因字符串。
pub const SKIP_PLAN_CACHE_REASON_SYNC_LOAD_FALLBACK: &str =
    "sync-load timed out and fell back to pseudo stats";
/// 收集谓词列并写入 `plan.used_stats` 的逻辑优化规则。
pub struct CollectPredicateColumnsPoint {
    /// 是否同时收集索引裁剪相关列。
    pub collect_index_pruning_columns: bool,
}
impl LogicalRule for CollectPredicateColumnsPoint {
    /// 规则注册名。
    fn name(&self) -> &'static str {
        "collect_predicate_columns_point"
    }
    /// 收集列统计使用情况，按访问表填充 UsedStats 后写回计划。
    fn optimize(&self, mut plan: Plan) -> Result<(Plan, bool), String> {
        let usage = collect_column_stats_usage(&plan, self.collect_index_pruning_columns);
        let mut by_table: BTreeMap<i64, UsedStats> = BTreeMap::new();
        for table_id in usage.visited_tables {
            let columns = columns_for_table(&plan, table_id, &usage.predicate_columns);
            by_table.insert(
                table_id,
                UsedStats {
                    table_id,
                    columns: columns.clone(),
                    indexes: BTreeSet::new(),
                    full_load: columns.iter().any(|column| {
                        usage
                            .predicate_columns
                            .get(column)
                            .copied()
                            .unwrap_or(false)
                    }),
                    pseudo: false,
                    version: 0,
                },
            );
        }
        plan.used_stats = by_table;
        Ok((plan, false))
    }
}

/// Restrict the global expression-column collection to columns produced by a
/// particular DataSource. A plan can contain multiple tables with overlapping
/// column IDs, so each table must request only its own statistics.
fn columns_for_table(
    plan: &Plan,
    table_id: i64,
    predicate_columns: &BTreeMap<i64, bool>,
) -> BTreeSet<i64> {
    let mut columns = BTreeSet::new();
    if let crate::rule_init::PlanKind::DataSource {
        table_id: current, ..
    } = &plan.kind
    {
        if *current == table_id {
            columns.extend(
                plan.schema
                    .iter()
                    .copied()
                    .filter(|column| predicate_columns.contains_key(column)),
            );
        }
    }
    for child in &plan.children {
        columns.extend(columns_for_table(child, table_id, predicate_columns));
    }
    columns
}

/// 统计信息加载器抽象：按 (表 ID, 列/索引 ID) 列表同步拉取直方图等。
pub trait StatsLoader {
    fn load(&self, items: &[(i64, i64)], wait_ms: u64) -> Result<BTreeSet<(i64, i64)>, String>;
}
/// 请求加载统计；若未能全部加载成功则返回跳过计划缓存的回退原因。
pub fn request_load_stats(
    loader: &dyn StatsLoader,
    needed: &[(i64, i64)],
    wait_ms: u64,
) -> Result<(), String> {
    let loaded = loader.load(needed, wait_ms)?;
    if needed.iter().all(|item| loaded.contains(item)) {
        Ok(())
    } else {
        Err(SKIP_PLAN_CACHE_REASON_SYNC_LOAD_FALLBACK.to_string())
    }
}
/// 收集直接依赖待加载列的虚拟列统计加载项。
///
/// 对应 Go `CollectDependingVirtualCols`：按表对待加载普通列分组，遍历公开虚拟列的
/// `ColumnInfo.Dependences`，只返回新发现的直接依赖虚拟列；输入列、索引项以及间接依赖
/// 均不包含在结果中。
pub fn collect_depending_virtual_columns(
    tables: &BTreeMap<i64, astersql_meta_model::TableInfo>,
    needed: &[astersql_meta_model::StatsLoadItem],
) -> Vec<astersql_meta_model::StatsLoadItem> {
    let mut needed_names = BTreeMap::<i64, BTreeSet<String>>::new();
    for item in needed.iter().filter(|item| !item.TableItemID.IsIndex) {
        let Some(table) = tables.get(&item.TableItemID.TableID) else {
            continue;
        };
        let Some(column) = table
            .Columns
            .iter()
            .find(|column| column.ID == item.TableItemID.ID)
        else {
            continue;
        };
        needed_names
            .entry(item.TableItemID.TableID)
            .or_default()
            .insert(column.Name.L.clone());
    }

    let mut generated = Vec::new();
    for (table_id, names) in needed_names {
        let Some(table) = tables.get(&table_id) else {
            continue;
        };
        for column in &table.Columns {
            if column.State != astersql_meta_model::StatePublic
                || !column.IsVirtualGenerated()
                || names.contains(&column.Name.L)
                || !column
                    .Dependences
                    .keys()
                    .any(|dependency| names.contains(dependency))
            {
                continue;
            }
            generated.push(astersql_meta_model::StatsLoadItem {
                TableItemID: astersql_meta_model::TableItemID {
                    TableID: table_id,
                    ID: column.ID,
                    IsIndex: false,
                    IsSyncLoadFailed: false,
                },
                FullLoad: true,
            });
        }
    }
    generated
}
