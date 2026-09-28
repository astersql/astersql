// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 自动 ANALYZE 作业工厂：根据表/分区统计指标决定是否创建分析作业。
//
// 对应 Go `analysis_job_factory.go`。按非分区表、静态分区、动态分区三种形态
// 计算变化率、表规模、距上次 ANALYZE 时长，并识别缺少统计的索引，
// 最终产出可入优先级队列的 `AnalysisJob`。

use crate::dynamic_partitioned_table_analysis_job::NewDynamicPartitionedTableAnalysisJob;
use crate::job::{AnalysisDuration, AnalysisJob};
use crate::non_partitioned_table_analysis_job::NewNonPartitionedTableAnalysisJob;
use crate::static_partitioned_table_analysis_job::NewStaticPartitionTableAnalysisJob;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 从未 ANALYZE 过的表使用的默认变化率（视为 100%，优先分析）。
pub const UNANALYZED_TABLE_DEFAULT_CHANGE_PERCENTAGE: f64 = 1.0;
/// 从未 ANALYZE 过的表假定“距上次分析”为 30 分钟，避免权重过低。
pub const UNANALYZED_TABLE_DEFAULT_LAST_UPDATE_DURATION: AnalysisDuration =
    AnalysisDuration::from_secs(-30 * 60);

/// 会话侧上下文：携带请求的统计版本（AnalyzeVersion）。
#[derive(Clone, Debug, Default)]
pub struct SessionContext {
    /// 会话请求的 ANALYZE 统计版本（如 Version1 / Version2）。
    pub analyze_version: i32,
}

/// 表上的索引元信息，用于判断是否缺统计。
#[derive(Clone, Debug, Default)]
pub struct IndexInfo {
    /// 索引 ID。
    pub id: i64,
    /// 索引名。
    pub name: String,
    /// 是否已对查询可见（public 状态）。
    pub is_public: bool,
    /// 是否为列存/向量类索引（自动分析时跳过）。
    pub is_columnar: bool,
    /// 是否为特殊全局索引（分区表场景下跳过）。
    pub is_special_global: bool,
}

/// 表元信息：表 ID 与索引列表。
#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    /// 表 ID。
    pub id: i64,
    /// 表上的索引集合。
    pub indices: Vec<IndexInfo>,
}

/// 表/分区级统计快照，供工厂计算变化率与规模。
#[derive(Clone, Debug, Default)]
pub struct TableStats {
    /// 实时行数估计（modify meta 维护的 realtime_count）。
    pub realtime_count: i64,
    /// 自上次 ANALYZE 以来的修改行数累计。
    pub modify_count: i64,
    /// 上次 ANALYZE 对应的版本时间戳（TSO）。
    pub last_analyze_version: u64,
    /// 上次 ANALYZE 时记录的行数，优先用作分母。
    pub analyze_row_count: i64,
    /// 列数，用于估算表规模（行数 × 列数）。
    pub column_count: usize,
    /// 当前统计信息版本号。
    pub stats_version: i32,
    /// 是否满足自动分析资格（如行数超过 AutoAnalyzeMinCnt）。
    pub eligible: bool,
    /// 是否为优化器估算出的伪统计；伪统计不携带待重写的分析版本。
    pub pseudo: bool,
    /// 是否已有有效 ANALYZE 结果。
    pub analyzed: bool,
    /// 已存在直方图/统计的索引 ID 集合。
    pub index_stats: HashSet<i64>,
    /// 已标记为“已分析”的列/索引 ID 集合。
    pub analyzed_ids: HashSet<i64>,
}

/// 分区标识：名称 + 物理表 ID。
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct PartitionIDAndName {
    /// 分区名。
    pub name: String,
    /// 分区物理表 ID。
    pub id: i64,
}

/// 分区定义（来自 schema）。
#[derive(Clone, Debug)]
pub struct PartitionDefinition {
    /// 分区物理表 ID。
    pub id: i64,
    /// 分区名。
    pub name: String,
}

/// 按分区 ID 拉取非伪统计（非 pseudo）的物理表统计。
pub trait PartitionStatsProvider {
    /// 返回指定物理表 ID 的真实统计；无则返回 `None`。
    fn get_non_pseudo_physical_table_stats(&self, id: i64) -> Option<TableStats>;
}

/// 分析作业工厂：持有会话版本、自动分析阈值与当前时间戳。
pub struct AnalysisJobFactory {
    /// 会话上下文（含 AnalyzeVersion）。
    pub sctx: SessionContext,
    /// 自动分析触发阈值：modify_count / 行数 超过该比例才建作业。
    pub auto_analyze_ratio: f64,
    /// 当前 TSO，用于计算距上次 ANALYZE 的时长。
    pub current_ts: u64,
}

/// 构造 `AnalysisJobFactory`。
pub fn NewAnalysisJobFactory(
    sctx: SessionContext,
    auto_analyze_ratio: f64,
    current_ts: u64,
) -> AnalysisJobFactory {
    AnalysisJobFactory {
        sctx,
        auto_analyze_ratio,
        current_ts,
    }
}

impl AnalysisJobFactory {
    /// 为非分区表创建分析作业；不合格、无变化且无缺索引时返回 `None`。
    pub fn CreateNonPartitionedTableAnalysisJob(
        &self,
        info: &TableInfo,
        stats: Option<&TableStats>,
    ) -> Option<Box<dyn AnalysisJob>> {
        let stats = stats?;
        if !stats.eligible {
            return None;
        }
        let requested_version = self.sctx.analyze_version;
        let version_matches = self.AnalyzeVersionMatches(stats);
        let change = self.CalculateChangePercentage(stats);
        let indexes = self.CheckIndexesNeedAnalyze(info, stats);
        // 变化率未超阈值且无新增索引时不入队。
        if change == 0.0 && indexes.is_empty() {
            return None;
        }
        Some(Box::new(NewNonPartitionedTableAnalysisJob(
            info.id,
            indexes,
            requested_version,
            !version_matches,
            change,
            self.CalculateTableSize(stats),
            self.GetTableLastAnalyzeDuration(stats),
        )))
    }

    /// 为静态分区裁剪模式下的单个分区创建分析作业。
    pub fn CreateStaticPartitionAnalysisJob(
        &self,
        info: &TableInfo,
        partition_id: i64,
        stats: Option<&TableStats>,
    ) -> Option<Box<dyn AnalysisJob>> {
        let stats = stats?;
        if !stats.eligible {
            return None;
        }
        let requested_version = self.sctx.analyze_version;
        let version_matches = self.AnalyzeVersionMatches(stats);
        let change = self.CalculateChangePercentage(stats);
        let indexes = self.CheckIndexesNeedAnalyze(info, stats);
        if change == 0.0 && indexes.is_empty() {
            return None;
        }
        Some(Box::new(NewStaticPartitionTableAnalysisJob(
            info.id,
            partition_id,
            indexes,
            requested_version,
            !version_matches,
            change,
            self.CalculateTableSize(stats),
            self.GetTableLastAnalyzeDuration(stats),
        )))
    }

    /// 为动态分区裁剪模式下的分区表创建作业：汇总超阈值分区与缺统计索引。
    pub fn CreateDynamicPartitionedTableAnalysisJob(
        &self,
        info: &TableInfo,
        global: Option<&TableStats>,
        partitions: &HashMap<PartitionIDAndName, TableStats>,
    ) -> Option<Box<dyn AnalysisJob>> {
        let global = global?;
        if !global.eligible {
            return None;
        }
        let requested_version = self.sctx.analyze_version;
        let version_matches = self.PartitionedTableAnalyzeVersionMatches(global, partitions);
        let (change, size, duration, ids) =
            self.CalculateIndicatorsForPartitions(global, partitions);
        let indexes = self.CheckNewlyAddedIndexesNeedAnalyzeForPartitionedTable(info, partitions);
        // 既无超阈值分区也无新增索引时不建作业。
        if ids.is_empty() && indexes.is_empty() {
            return None;
        }
        Some(Box::new(NewDynamicPartitionedTableAnalysisJob(
            info.id,
            ids,
            indexes,
            requested_version,
            !version_matches,
            change,
            size,
            duration,
        )))
    }

    /// 统计版本是否与会话请求一致；伪统计或 version 0 无需重写告警。
    pub fn AnalyzeVersionMatches(&self, stats: &TableStats) -> bool {
        debug_assert_eq!(
            self.sctx.analyze_version, 2,
            "requested analyze version should be 2"
        );
        stats.pseudo || stats.stats_version == 0 || stats.stats_version == self.sctx.analyze_version
    }

    /// 全局统计与所有分区统计的版本均与会话请求一致。
    pub fn PartitionedTableAnalyzeVersionMatches(
        &self,
        global: &TableStats,
        partitions: &HashMap<PartitionIDAndName, TableStats>,
    ) -> bool {
        self.AnalyzeVersionMatches(global)
            && partitions
                .values()
                .all(|stats| self.AnalyzeVersionMatches(stats))
    }

    /// 计算变化率：未分析表返回默认 1.0；已分析表为 modify/行数，未超阈值则返回 0。
    pub fn CalculateChangePercentage(&self, stats: &TableStats) -> f64 {
        if !stats.analyzed {
            return UNANALYZED_TABLE_DEFAULT_CHANGE_PERCENTAGE;
        }
        // ratio=0 表示关闭基于变化率的自动分析。
        if self.auto_analyze_ratio == 0.0 {
            return 0.0;
        }
        // 优先用上次 ANALYZE 行数作分母，否则回退 realtime_count。
        let table_count = if stats.analyze_row_count > 0 {
            stats.analyze_row_count
        } else {
            stats.realtime_count
        };
        let result = stats.modify_count as f64 / table_count as f64;
        if result > self.auto_analyze_ratio {
            result
        } else {
            0.0
        }
    }

    /// 估算表规模：实时行数 × 列数，供优先级权重使用。
    pub fn CalculateTableSize(&self, stats: &TableStats) -> f64 {
        assert!(stats.column_count != 0, "Column count should not be 0");
        stats.realtime_count as f64 * stats.column_count as f64
    }

    /// 距上次 ANALYZE 的时长。
    pub fn GetTableLastAnalyzeDuration(&self, stats: &TableStats) -> AnalysisDuration {
        let nanos = match self
            .CurrentTime()
            .duration_since(self.FindLastAnalyzeTime(stats))
        {
            Ok(duration) => duration.as_nanos() as i128,
            Err(error) => -(error.duration().as_nanos() as i128),
        };
        // Go Time.Sub saturates when the result exceeds a time.Duration.
        AnalysisDuration::from_nanos(nanos.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
    }

    /// 上次 ANALYZE 时间；未分析表回退为当前时间减去默认 30 分钟。
    pub fn FindLastAnalyzeTime(&self, stats: &TableStats) -> SystemTime {
        if !stats.analyzed {
            return self
                .CurrentTime()
                .checked_sub(Duration::from_nanos(
                    UNANALYZED_TABLE_DEFAULT_LAST_UPDATE_DURATION
                        .as_nanos()
                        .unsigned_abs(),
                ))
                .unwrap_or(UNIX_EPOCH);
        }
        oracle_time(stats.last_analyze_version)
    }

    /// 将工厂持有的当前 TSO 转为 `SystemTime`。
    fn CurrentTime(&self) -> SystemTime {
        oracle_time(self.current_ts)
    }

    /// 返回已分析表上仍缺统计的 public 非列存索引 ID 集合。
    pub fn CheckIndexesNeedAnalyze(
        &self,
        info: &TableInfo,
        stats: &TableStats,
    ) -> HashMap<i64, ()> {
        // 未分析表会整表 ANALYZE，无需单独列出索引。
        if !stats.analyzed {
            return HashMap::new();
        }
        info.indices
            .iter()
            .filter(|index| {
                index.is_public
                    && !index.is_columnar
                    && !stats.index_stats.contains(&index.id)
                    && !stats.analyzed_ids.contains(&index.id)
            })
            .map(|index| (index.id, ()))
            .collect()
    }

    /// 汇总超阈值分区的平均变化率、平均规模、平均距上次分析时长及分区 ID 集合。
    pub fn CalculateIndicatorsForPartitions(
        &self,
        global: &TableStats,
        partitions: &HashMap<PartitionIDAndName, TableStats>,
    ) -> (f64, f64, AnalysisDuration, HashMap<i64, ()>) {
        assert!(global.column_count != 0, "Column count should not be 0");
        let mut change = 0.0;
        let mut size = 0.0;
        let mut duration = 0_i64;
        let mut ids = HashMap::new();
        for (partition, stats) in partitions {
            let part_change = self.CalculateChangePercentage(stats);
            if part_change == 0.0 {
                continue;
            }
            change += part_change;
            // 分区规模用全局列数估算，与 Go 一致。
            size += stats.realtime_count as f64 * global.column_count as f64;
            duration = duration.wrapping_add(self.GetTableLastAnalyzeDuration(stats).as_nanos());
            ids.insert(partition.id, ());
        }
        if ids.is_empty() {
            return (0.0, 0.0, AnalysisDuration::ZERO, ids);
        }
        let count = ids.len();
        (
            change / count as f64,
            size / count as f64,
            AnalysisDuration::from_nanos(duration / count as i64),
            ids,
        )
    }

    /// 识别分区表上新增且缺统计的索引，返回 `索引 ID -> 缺统计的分区 ID 列表`。
    pub fn CheckNewlyAddedIndexesNeedAnalyzeForPartitionedTable(
        &self,
        info: &TableInfo,
        partitions: &HashMap<PartitionIDAndName, TableStats>,
    ) -> HashMap<i64, Vec<i64>> {
        let mut result = HashMap::new();
        for index in &info.indices {
            // 跳过非 public、列存/向量索引与特殊全局索引。
            if !index.is_public || index.is_columnar || index.is_special_global {
                continue;
            }
            let ids = partitions
                .iter()
                .filter_map(|(partition, stats)| {
                    (!stats.index_stats.contains(&index.id)
                        && !stats.analyzed_ids.contains(&index.id))
                    .then_some(partition.id)
                })
                .collect::<Vec<_>>();
            if !ids.is_empty() {
                result.insert(index.id, ids);
            }
        }
        result
    }
}

/// 由分区名与 ID 构造 `PartitionIDAndName`。
pub fn NewPartitionIDAndName(name: String, id: i64) -> PartitionIDAndName {
    PartitionIDAndName { name, id }
}

/// 按分区定义批量拉取合格的非伪物理表统计。
pub fn GetPartitionStats(
    handle: &dyn PartitionStatsProvider,
    definitions: &[PartitionDefinition],
) -> HashMap<PartitionIDAndName, TableStats> {
    definitions
        .iter()
        .filter_map(|definition| {
            let stats = handle.get_non_pseudo_physical_table_stats(definition.id)?;
            stats.eligible.then(|| {
                (
                    NewPartitionIDAndName(definition.name.clone(), definition.id),
                    stats,
                )
            })
        })
        .collect()
}

/// 自动分析允许执行的日内时间窗口（可跨午夜）。
#[derive(Clone, Copy, Debug)]
pub struct AutoAnalysisTimeWindow {
    /// 窗口开始时刻。
    pub start: SystemTime,
    /// 窗口结束时刻。
    pub end: SystemTime,
}

/// 构造自动分析时间窗口。
pub fn NewAutoAnalysisTimeWindow(start: SystemTime, end: SystemTime) -> AutoAnalysisTimeWindow {
    AutoAnalysisTimeWindow { start, end }
}

impl AutoAnalysisTimeWindow {
    /// 判断 `current` 是否落在日内窗口内；`start > end` 表示跨午夜。
    pub fn IsWithinTimeWindow(&self, current: SystemTime) -> bool {
        // `SystemTime` 没有 Go `time.Time{}` 的 year-1 零值；本移植以
        // `UNIX_EPOCH` 作为未配置哨兵，并保持 Go 的零窗口拒绝语义。
        if self.start == UNIX_EPOCH || self.end == UNIX_EPOCH {
            return false;
        }
        // Go `WithinDayTimePeriod` 只保留 UTC 小时与分钟，忽略日期和秒。
        let seconds = |time: SystemTime| {
            time.duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                / 60
                % (24 * 60)
        };
        let start = seconds(self.start);
        let end = seconds(self.end);
        let current = seconds(current);
        if start <= end {
            current >= start && current <= end
        } else {
            // 跨午夜：落在 [start, 1440) 或 [0, end] 均视为命中。
            current >= start || current <= end
        }
    }
}

/// 将 TiDB TSO 转为物理时间。
fn oracle_time(timestamp: u64) -> SystemTime {
    // TiDB TSO stores physical milliseconds in the high bits and a logical counter in the low 18 bits.
    // 高位为物理毫秒，低 18 位为逻辑计数器。
    UNIX_EPOCH + Duration::from_millis(timestamp >> 18)
}
