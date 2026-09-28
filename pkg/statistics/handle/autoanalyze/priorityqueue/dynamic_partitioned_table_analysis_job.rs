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

// 动态分区裁剪模式下的分区表分析作业。
//
// 对应 Go `dynamic_partitioned_table_analysis_job.go`。将超阈值分区与缺统计索引
// 批量拼成 `ANALYZE TABLE ... PARTITION ...` SQL，经 `AnalysisRuntime` 执行，
// 并支持成功/失败钩子与 JSON/Display 序列化。

use crate::job::*;
use std::any::Any;
use std::collections::HashMap;
use std::fmt;

/// 分析类型：动态分区整表（列+索引）ANALYZE。
pub const ANALYZE_DYNAMIC_PARTITION: &str = "analyzeDynamicPartition";
/// 分析类型：仅为动态分区上的新增索引做 ANALYZE。
pub const ANALYZE_DYNAMIC_PARTITION_INDEX: &str = "analyzeDynamicPartitionIndex";

/// 动态分区表分析作业状态与执行参数。
pub struct DynamicPartitionedTableAnalysisJob {
    /// 成功完成时回调（参数为全局表 ID）。
    success_hook: Option<SuccessJobHook>,
    /// 失败时回调（参数为全局表 ID 与是否可重试类失败）。
    failure_hook: Option<FailureJobHook>,
    /// 逻辑表（全局）ID。
    pub GlobalTableID: i64,
    /// 需要 ANALYZE 的分区物理 ID 集合。
    pub PartitionIDs: HashMap<i64, ()>,
    /// 缺统计索引：索引 ID → 相关分区物理 ID 列表。
    pub PartitionIndexIDs: HashMap<i64, Vec<i64>>,
    /// 优先级指标（变化率、规模、距上次分析时长）。
    pub indicators: Indicators,
    /// 目标统计版本。
    pub TableStatsVer: i32,
    /// 是否需要旧版本统计重写告警。
    pub NeedVersionRewriteWarn: bool,
    /// 队列优先级权重。
    pub Weight: f64,
    /// schema 名（ValidateAndPrepare 填充）。
    pub SchemaName: String,
    /// 逻辑表名（ValidateAndPrepare 填充）。
    pub GlobalTableName: String,
    /// 待分析分区名列表。
    pub PartitionNames: Vec<String>,
    /// 待分析索引名 → 相关分区名列表。
    pub PartitionIndexNames: HashMap<String, Vec<String>>,
}

/// 构造动态分区分析作业；权重与名称字段在入队/校验阶段再填充。
#[allow(clippy::too_many_arguments)]
pub fn NewDynamicPartitionedTableAnalysisJob(
    table_id: i64,
    partition_ids: HashMap<i64, ()>,
    partition_index_ids: HashMap<i64, Vec<i64>>,
    table_stats_ver: i32,
    need_version_rewrite_warn: bool,
    change_percentage: f64,
    table_size: f64,
    last_analysis_duration: impl Into<AnalysisDuration>,
) -> DynamicPartitionedTableAnalysisJob {
    DynamicPartitionedTableAnalysisJob {
        success_hook: None,
        failure_hook: None,
        GlobalTableID: table_id,
        PartitionIDs: partition_ids,
        PartitionIndexIDs: partition_index_ids,
        indicators: Indicators {
            ChangePercentage: change_percentage,
            TableSize: table_size,
            LastAnalysisDuration: last_analysis_duration.into(),
        },
        TableStatsVer: table_stats_ver,
        NeedVersionRewriteWarn: need_version_rewrite_warn,
        Weight: 0.0,
        SchemaName: String::new(),
        GlobalTableName: String::new(),
        PartitionNames: Vec::new(),
        PartitionIndexNames: HashMap::new(),
    }
}

/// 生成带若干 `%n` 占位符的分区 ANALYZE SQL 模板片段。
pub fn GetPartitionSQL(prefix: &str, suffix: &str, count: usize) -> String {
    let placeholders = std::iter::repeat_n("%n", count)
        .collect::<Vec<_>>()
        .join(", ");
    if placeholders.is_empty() {
        format!("{prefix}{suffix}")
    } else {
        format!("{prefix} {placeholders}{suffix}")
    }
}

/// 从「索引名 → 分区名列表」中展平所有分区名。
pub fn GetPartitionNames(indexes: &HashMap<String, Vec<String>>) -> Vec<String> {
    indexes.values().flatten().cloned().collect()
}

impl DynamicPartitionedTableAnalysisJob {
    /// 按是否含新增索引返回分析类型常量。
    pub fn GetAnalyzeType(&self) -> &'static str {
        if self.PartitionIndexIDs.is_empty() {
            ANALYZE_DYNAMIC_PARTITION
        } else {
            ANALYZE_DYNAMIC_PARTITION_INDEX
        }
    }
    /// 按成败调用已注册钩子。
    fn finish(&mut self, success: bool) {
        if success {
            if let Some(hook) = self.success_hook.clone() {
                hook(self);
            }
        } else if let Some(hook) = self.failure_hook.clone() {
            hook(self, true);
        }
    }
    /// 按 runtime 批大小分批对分区执行整表 ANALYZE。
    fn AnalyzePartitions(&self, runtime: &dyn AnalysisRuntime) -> Result<bool, String> {
        let batch_size = runtime.partition_batch_size().max(1);
        for names in self.PartitionNames.chunks(batch_size) {
            let sql = GetPartitionSQL("analyze table %n.%n partition", "", names.len());
            let mut params = vec![self.SchemaName.clone(), self.GlobalTableName.clone()];
            params.extend_from_slice(names);
            if !runtime.execute_analyze(
                &sql,
                &params,
                self.TableStatsVer,
                self.NeedVersionRewriteWarn,
            )? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// 按批对分区执行索引 ANALYZE。
    fn AnalyzePartitionIndexes(&self, runtime: &dyn AnalysisRuntime) -> Result<bool, String> {
        let batch_size = runtime.partition_batch_size().max(1);
        // Statistics version 2 refreshes columns and all indexes, so Go deliberately analyzes only the first index.
        // 统计版本 2 会刷新列与全部索引，故与 Go 一样只对第一个索引发 ANALYZE。
        if let Some((index, partition_names)) = self.PartitionIndexNames.iter().next() {
            for names in partition_names.chunks(batch_size) {
                let sql =
                    GetPartitionSQL("analyze table %n.%n partition", " index %n", names.len());
                let mut params = vec![self.SchemaName.clone(), self.GlobalTableName.clone()];
                params.extend_from_slice(names);
                params.push(index.clone());
                if !runtime.execute_analyze(
                    &sql,
                    &params,
                    self.TableStatsVer,
                    self.NeedVersionRewriteWarn,
                )? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

impl AnalysisJob for DynamicPartitionedTableAnalysisJob {
    /// 校验表仍为分区表，解析分区/索引名，并确认当前可执行 ANALYZE。
    fn ValidateAndPrepare(&mut self, runtime: &dyn AnalysisRuntime) -> (bool, String) {
        let Some(metadata) = runtime.table_by_id(self.GlobalTableID) else {
            if let Some(hook) = self.failure_hook.clone() {
                hook(self, false);
            }
            return (false, TABLE_NOT_EXIST.to_owned());
        };
        if metadata.partitions.is_empty() {
            if let Some(hook) = self.failure_hook.clone() {
                hook(self, false);
            }
            return (false, NOT_PARTITIONED_TABLE.to_owned());
        }
        self.SchemaName = metadata.schema_name.clone();
        self.GlobalTableName = metadata.table_name.clone();
        // 物理分区 ID → 分区名，供后续 SQL 参数使用。
        let partition_names_by_id = metadata
            .partitions
            .iter()
            .map(|partition| (partition.id, partition.name.clone()))
            .collect::<HashMap<_, _>>();
        self.PartitionNames = self
            .PartitionIDs
            .keys()
            .filter_map(|id| partition_names_by_id.get(id).cloned())
            .collect();
        self.PartitionIndexNames.clear();
        for index in &metadata.indices {
            if let Some(partition_ids) = self.PartitionIndexIDs.get(&index.id) {
                let names = partition_ids
                    .iter()
                    .filter_map(|id| partition_names_by_id.get(id).cloned())
                    .collect::<Vec<_>>();
                if !names.is_empty() {
                    self.PartitionIndexNames.insert(index.name.clone(), names);
                }
            }
        }
        if self.PartitionNames.is_empty() && self.PartitionIndexNames.is_empty() {
            return (true, String::new());
        }
        let mut names = self.PartitionNames.clone();
        names.extend(GetPartitionNames(&self.PartitionIndexNames));
        let result = IsValidToAnalyze(runtime, &self.SchemaName, &self.GlobalTableName, &names);
        if !result.0
            && let Some(hook) = self.failure_hook.clone()
        {
            hook(self, true);
        }
        result
    }
    /// 执行分区或分区索引 ANALYZE，并触发成功/失败钩子。
    fn Analyze(&mut self, runtime: &dyn AnalysisRuntime) -> Result<(), String> {
        let result = if self.PartitionIndexIDs.is_empty() {
            self.AnalyzePartitions(runtime)
        } else {
            self.AnalyzePartitionIndexes(runtime)
        };
        match result {
            Ok(success) => {
                self.finish(success);
                Ok(())
            }
            Err(error) => {
                self.finish(false);
                Err(error)
            }
        }
    }
    fn SetWeight(&mut self, weight: f64) {
        self.Weight = weight;
    }
    fn GetWeight(&self) -> f64 {
        self.Weight
    }
    /// 是否包含待分析的新增索引。
    fn HasNewlyAddedIndex(&self) -> bool {
        !self.PartitionIndexIDs.is_empty()
    }
    fn GetIndicators(&self) -> Indicators {
        self.indicators.clone()
    }
    fn SetIndicators(&mut self, indicators: Indicators) {
        self.indicators = indicators;
    }
    fn GetTableID(&self) -> i64 {
        self.GlobalTableID
    }
    fn RegisterSuccessHook(&mut self, hook: SuccessJobHook) {
        self.success_hook = Some(hook);
    }
    fn RegisterFailureHook(&mut self, hook: FailureJobHook) {
        self.failure_hook = Some(hook);
    }
    /// 导出可观测用的 JSON 视图。
    fn AsJSON(&self) -> AnalysisJobJSON {
        AnalysisJobJSON {
            Type: self.GetAnalyzeType().to_owned(),
            TableID: self.GlobalTableID,
            SchemaName: self.SchemaName.clone(),
            TableName: self.GlobalTableName.clone(),
            PartitionNames: self.PartitionNames.clone(),
            IndexNames: self.PartitionIndexNames.keys().cloned().collect(),
            Indicators: AsJSONIndicators(&self.indicators),
            Weight: format!("{:.6}", self.Weight),
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl fmt::Display for DynamicPartitionedTableAnalysisJob {
    /// 人类可读的作业摘要（与 Go String 格式对齐）。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "DynamicPartitionedTableAnalysisJob:\n\tAnalyzeType: {}\n\tPartitions: {}\n\tPartitionIndexes: {}\n\tSchema: {}\n\tGlobal Table: {}\n\tGlobal TableID: {}\n\tTableStatsVer: {}\n\tChangePercentage: {:.6}\n\tTableSize: {:.2}\n\tLastAnalysisDuration: {}\n\tWeight: {:.6}\n",
            self.GetAnalyzeType(),
            self.PartitionNames.join(", "),
            crate::job::format_go_string_list_map(&self.PartitionIndexNames),
            self.SchemaName,
            self.GlobalTableName,
            self.GlobalTableID,
            self.TableStatsVer,
            self.indicators.ChangePercentage,
            self.indicators.TableSize,
            crate::job::format_go_duration(self.indicators.LastAnalysisDuration),
            self.Weight
        )
    }
}
