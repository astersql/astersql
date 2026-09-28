// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 表级统计信息加载与伪统计（Pseudo Stats）构造。
//
// 对应 Go 侧 `GetStatsTable` / `LoadTableStats`：在统计句柄未初始化、行数为 0
// 或统计过期时返回伪表；确定性优化目标（OptObjectiveDeterminate）会清零实时
// 修改量，仅使用 ANALYZE 行数。UsedStats 记录语句实际引用的统计快照。
//

// 依赖仅作为接线点。类型断言、nil 检查、命名返回值及错误传播语义在附近保留原结构。
//
//
// Go imports 已保留为外部依赖清单，当前不解析这些导入。
//
// GetTblInfoForUsedStatsByPhysicalID get table name, partition name and HintedTable
// that will be used to record used stats.
// GetTblInfoForUsedStatsByPhysicalID 对应 Go 中的同名函数；Rust 侧通过 StatsSource 抽象统计与元数据访问。
// pub fn GetTblInfoForUsedStatsByPhysicalID(sctx base.PlanContext, id int64) (
//     fullName string, tblInfo *model.TableInfo) {
//     fullName = "tableID " + strconv.FormatInt(id, 10)
//
//     is := sctx.GetLatestISWithoutSessExt()
//     tbl, partDef := infoschema.FindTableByTblOrPartID(is.(infoschema.InfoSchema), id)
//     if tbl == nil || tbl.Meta() == nil {
//         return
//     }
//     tblInfo = tbl.Meta()
//     fullName = tblInfo.Name.O
//     if partDef != nil {
//         fullName += " " + partDef.Name.O
//     } else if pi := tblInfo.GetPartitionInfo(); pi != nil && len(pi.Definitions) > 0 {
//         fullName += " global"
//     }
//     return
// }
//
// GetStatsTable gets statistics information for a table specified by "tableID".
// A pseudo statistics table is returned in any of the following scenario:
// 1. tidb-server started and statistics handle has not been initialized.
// 2. table row count from statistics is zero.
// 3. statistics is outdated.
// Note: please also update getLatestVersionFromStatsTable() when logic in this function changes.
// GetStatsTable 对应 Go 中的同名函数；Rust 侧通过 StatsSource 注入统计句柄语义。
// pub fn GetStatsTable(ctx base.PlanContext, tblInfo *model.TableInfo, pid int64) *statistics.Table {
// 先处理统计句柄未初始化和实时行数为零的分支：Go 会返回 pseudo table，保留同一短路顺序。
//     var statsHandle *handle.Handle
//     dom := domain.GetDomain(ctx)
//     if dom != nil {
//         statsHandle = dom.StatsHandle()
//     }
//     var pseudoStatsForUninitialized, pseudoStatsForOutdated bool
//     var statsTbl *statistics.Table
// 1. tidb-server started and statistics handle has not been initialized.
//     if statsHandle == nil {
//         return statistics.PseudoTable(tblInfo, false, true)
//     }
//
//     if pid == tblInfo.ID || ctx.GetSessionVars().StmtCtx.UseDynamicPartitionPrune() {
//         statsTbl = statsHandle.GetPhysicalTableStats(tblInfo.ID, tblInfo)
//     } else {
//         statsTbl = statsHandle.GetPhysicalTableStats(pid, tblInfo)
//     }
//     intest.Assert(statsTbl.ColAndIdxExistenceMap != nil, "The existence checking map must not be nil.")
//
//     allowPseudoTblTriggerLoading := false
// In OptObjectiveDeterminate mode, we need to ignore the real-time stats.
// To achieve this, we copy the statsTbl and reset the real-time stats fields (set ModifyCount to 0 and set
// RealtimeCount to the row count from the ANALYZE, which is fetched from loaded stats in GetAnalyzeRowCount()).
//     if ctx.GetSessionVars().GetOptObjective() == vardef.OptObjectiveDeterminate {
// 确定性优化目标会复制统计表并清零实时修改量，避免直接改动缓存对象。
//         analyzeCount := max(int64(statsTbl.GetAnalyzeRowCount()), 0)
// If the two fields are already the values we want, we don't need to modify it, and also we don't need to copy.
//         if statsTbl.RealtimeCount != analyzeCount || statsTbl.ModifyCount != 0 {
// Here is a case that we need specially care about:
// The original stats table from the stats cache is not a pseudo table, but the analyze row count is 0 (probably
// because of no col/idx stats are loaded), which will makes it a pseudo table according to the rule 2 below.
// Normally, a pseudo table won't trigger stats loading since we assume it means "no stats available", but
// in such case, we need it able to trigger stats loading.
// That's why we use the special allowPseudoTblTriggerLoading flag here.
//             if !statsTbl.Pseudo && statsTbl.RealtimeCount > 0 && analyzeCount == 0 {
//                 allowPseudoTblTriggerLoading = true
//             }
// Copy it so we can modify the ModifyCount and the RealtimeCount safely.
//             statsTbl = statsTbl.CopyAs(statistics.MetaOnly)
//             statsTbl.RealtimeCount = analyzeCount
//             statsTbl.ModifyCount = 0
//         }
//     }
//
// 2. table row count from statistics is zero.
//     if statsTbl.RealtimeCount == 0 {
//         core_metrics.PseudoEstimationNotAvailable.Inc()
//         return statistics.PseudoTable(tblInfo, allowPseudoTblTriggerLoading, true)
//     }
//
// 3. statistics is uninitialized or outdated.
//     pseudoStatsForUninitialized = !statsTbl.IsInitialized()
//     pseudoStatsForOutdated = ctx.GetSessionVars().GetEnablePseudoForOutdatedStats() && statsTbl.IsOutdated()
//     if pseudoStatsForUninitialized || pseudoStatsForOutdated {
//         tbl := *statsTbl
//         tbl.Pseudo = true
//         statsTbl = &tbl
//         if pseudoStatsForUninitialized {
//             core_metrics.PseudoEstimationNotAvailable.Inc()
//         } else {
//             core_metrics.PseudoEstimationOutdate.Inc()
//         }
//     }
//
//     return statsTbl
// }
//
// LoadTableStats loads the stats of the table and store it in the statement `UsedStatsInfo` if it didn't exist
// LoadTableStats 对应 Go 中的同名函数；Rust 侧将语句级记录显式建模为 BTreeMap。
// pub fn LoadTableStats(ctx sessionctx.Context, tblInfo *model.TableInfo, pid int64) {
//     statsRecord := ctx.GetSessionVars().StmtCtx.GetUsedStatsInfo(true)
//     if statsRecord.GetUsedInfo(pid) != nil {
//         return
//     }
//
//     pctx := ctx.GetPlanCtx()
//     tableStats := GetStatsTable(pctx, tblInfo, pid)
//
//     name := tblInfo.Name.O
//     partInfo := tblInfo.GetPartitionInfo()
//     if partInfo != nil {
//         for _, p := range partInfo.Definitions {
//             if p.ID == pid {
//                 name += " " + p.Name.O
//             }
//         }
//     }
//     usedStats := &stmtctx.UsedStatsInfoForTable{
//         Name:          name,
//         TblInfo:       tblInfo,
//         RealtimeCount: tableStats.HistColl.RealtimeCount,
//         ModifyCount:   tableStats.HistColl.ModifyCount,
//         Version:       tableStats.Version,
//     }
//     if tableStats.Pseudo {
//         usedStats.Version = statistics.PseudoVersion
//     }
//     statsRecord.RecordUsedInfo(pid, usedStats)
// }
// */
use std::collections::BTreeMap;

/// 伪统计版本号；伪表的 Version 固定为此值。
pub const PSEUDO_VERSION: u64 = 0;

/// 分区元数据：物理 ID 与分区名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionInfo {
    /// 分区物理 ID。
    pub id: i64,
    /// 分区名。
    pub name: String,
}
/// 表元数据：表 ID/名与分区列表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableInfo {
    /// 表物理 ID。
    pub id: i64,
    /// 表名。
    pub name: String,
    /// 分区定义列表；非空且无具体分区命中时名称带 global。
    pub partitions: Vec<PartitionInfo>,
}
/// 一张表（或分区）的统计快照，含实时行数、ANALYZE 行数与伪表标记。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatisticsTable {
    /// 实时估计行数（含增量修改）。
    pub realtime_count: i64,
    /// 自上次 ANALYZE 以来的修改行数。
    pub modify_count: i64,
    /// ANALYZE 得到的行数。
    pub analyze_count: i64,
    /// 统计版本；伪表为 PSEUDO_VERSION。
    pub version: u64,
    /// 是否已完成初始化（有可用直方图等）。
    pub initialized: bool,
    /// 是否判定为过期。
    pub outdated: bool,
    /// 是否为伪统计表。
    pub pseudo: bool,
    /// 伪表是否仍允许触发统计加载。
    pub allow_pseudo_loading: bool,
}

impl StatisticsTable {
    /// 构造伪统计表；`allow_loading` 控制是否可触发加载。
    pub fn pseudo(allow_loading: bool) -> Self {
        Self {
            realtime_count: 10_000,
            modify_count: 0,
            analyze_count: 0,
            version: PSEUDO_VERSION,
            initialized: false,
            outdated: false,
            pseudo: true,
            allow_pseudo_loading: allow_loading,
        }
    }
}

/// 统计数据源：按物理 ID 查表/分区，并取物理统计。
pub trait StatsSource {
    /// 按物理 ID 查找表与可选分区定义。
    fn table_by_physical_id(&self, id: i64) -> Option<(TableInfo, Option<PartitionInfo>)>;
    /// 读取指定物理 ID 的统计表。
    fn physical_stats(&self, physical_id: i64, table: &TableInfo) -> Option<StatisticsTable>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 优化目标：Default 使用实时统计；Determinate 仅用 ANALYZE 行数。
pub enum OptimizationObjective {
    Default,
    Determinate,
}

/// 为 UsedStats 记录解析表名（含分区或 global 后缀）与表元数据。
pub fn table_info_for_used_stats(source: &dyn StatsSource, id: i64) -> (String, Option<TableInfo>) {
    let Some((table, partition)) = source.table_by_physical_id(id) else {
        return (format!("tableID {id}"), None);
    };
    // 按分区命中、分区表全局、普通表三种情况拼显示名。
    let name = match partition {
        Some(partition) => format!("{} {}", table.name, partition.name),
        None if !table.partitions.is_empty() => format!("{} global", table.name),
        None => table.name.clone(),
    };
    (name, Some(table))
}

/// 获取表统计；句柄缺失、行数为 0 或过期时返回/标记伪表。
pub fn get_stats_table(
    source: Option<&dyn StatsSource>,
    table: &TableInfo,
    physical_id: i64,
    dynamic_partition_pruning: bool,
    objective: OptimizationObjective,
    pseudo_for_outdated: bool,
) -> StatisticsTable {
    // 无统计源时直接返回伪表。
    let Some(source) = source else {
        return StatisticsTable::pseudo(false);
    };
    // 动态分区裁剪或访问整表时使用表 ID，否则用分区物理 ID。
    let target_id = if physical_id == table.id || dynamic_partition_pruning {
        table.id
    } else {
        physical_id
    };
    let Some(mut stats) = source.physical_stats(target_id, table) else {
        return StatisticsTable::pseudo(false);
    };
    let mut allow_loading = false;
    // Determinate：用 analyze_count 覆盖 realtime，并清零 modify_count。
    if objective == OptimizationObjective::Determinate {
        let analyze = stats.analyze_count.max(0);
        if stats.realtime_count != analyze || stats.modify_count != 0 {
            allow_loading = !stats.pseudo && stats.realtime_count > 0 && analyze == 0;
            stats.realtime_count = analyze;
            stats.modify_count = 0;
        }
    }
    // 实时行数为 0 → 伪表（可能允许触发加载）。
    if stats.realtime_count == 0 {
        return StatisticsTable::pseudo(allow_loading);
    }
    // 未初始化或（开启开关且）过期 → 标记伪表。
    if !stats.initialized || (pseudo_for_outdated && stats.outdated) {
        stats.pseudo = true;
    }
    stats
}

/// 语句已使用的单表统计摘要，写入 StmtCtx.UsedStatsInfo。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsedStatsInfo {
    /// 显示名（表或「表 分区」）。
    pub name: String,
    /// 表元数据副本。
    pub table: TableInfo,
    /// 实时行数。
    pub realtime_count: i64,
    /// 修改行数。
    pub modify_count: i64,
    /// 统计版本。
    pub version: u64,
}

/// 若尚未记录该物理 ID，则加载统计并写入 UsedStats 映射。
pub fn load_table_stats(
    record: &mut BTreeMap<i64, UsedStatsInfo>,
    source: Option<&dyn StatsSource>,
    table: &TableInfo,
    physical_id: i64,
    dynamic_partition_pruning: bool,
    objective: OptimizationObjective,
    pseudo_for_outdated: bool,
) {
    // 已记录则跳过，避免重复加载。
    if record.contains_key(&physical_id) {
        return;
    }
    let stats = get_stats_table(
        source,
        table,
        physical_id,
        dynamic_partition_pruning,
        objective,
        pseudo_for_outdated,
    );
    // 若 physical_id 对应分区，显示名带分区后缀。
    let name = table
        .partitions
        .iter()
        .find(|partition| partition.id == physical_id)
        .map(|partition| format!("{} {}", table.name, partition.name))
        .unwrap_or_else(|| table.name.clone());
    record.insert(
        physical_id,
        UsedStatsInfo {
            name,
            table: table.clone(),
            realtime_count: stats.realtime_count,
            modify_count: stats.modify_count,
            version: if stats.pseudo {
                PSEUDO_VERSION
            } else {
                stats.version
            },
        },
    );
}
