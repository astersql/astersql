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

// 静态分区模式（static prune mode）下单分区的自动 ANALYZE 作业。
//
// 与动态分区不同：静态模式下每个分区作为独立物理表收集统计，
// 作业以 `StaticPartitionID` 作为队列键（GetTableID），互不影响失败冷却。

use crate::job::*;
use std::any::Any;
use std::collections::HashMap;
use std::fmt;

/// 作业类型：分析单个静态分区。
pub const ANALYZE_STATIC_PARTITION: &str = "analyzeStaticPartition";
/// 作业类型：分析单个静态分区上的指定索引。
pub const ANALYZE_STATIC_PARTITION_INDEX: &str = "analyzeStaticPartitionIndex";

/// 静态分区表分析作业：绑定全局表 + 单个分区（及可选索引）。
pub struct StaticPartitionedTableAnalysisJob {
    success_hook: Option<SuccessJobHook>,
    failure_hook: Option<FailureJobHook>,
    /// 全局逻辑表 ID。
    pub GlobalTableID: i64,
    /// 当前作业对应的分区物理 ID（亦作队列键）。
    pub StaticPartitionID: i64,
    /// 待分析索引 ID；空则分析整个分区。
    pub IndexIDs: HashMap<i64, ()>,
    /// 优先级指标。
    pub indicators: Indicators,
    /// 统计版本。
    pub TableStatsVer: i32,
    /// 是否需要旧版本统计重写告警。
    pub NeedVersionRewriteWarn: bool,
    /// 调度权重。
    pub Weight: f64,
    /// Schema 名（ValidateAndPrepare 填充）。
    pub SchemaName: String,
    /// 全局表名。
    pub GlobalTableName: String,
    /// 分区名。
    pub StaticPartitionName: String,
    /// 索引名列表。
    pub IndexNames: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
/// 构造静态分区分析作业；名称字段初始为空。
pub fn NewStaticPartitionTableAnalysisJob(
    global_table_id: i64,
    partition_id: i64,
    index_ids: HashMap<i64, ()>,
    table_stats_ver: i32,
    need_version_rewrite_warn: bool,
    change_percentage: f64,
    table_size: f64,
    last_analysis_duration: impl Into<AnalysisDuration>,
) -> StaticPartitionedTableAnalysisJob {
    StaticPartitionedTableAnalysisJob {
        success_hook: None,
        failure_hook: None,
        GlobalTableID: global_table_id,
        StaticPartitionID: partition_id,
        IndexIDs: index_ids,
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
        StaticPartitionName: String::new(),
        IndexNames: Vec::new(),
    }
}

impl StaticPartitionedTableAnalysisJob {
    /// 按是否有 IndexIDs 返回分区/分区索引分析类型。
    pub fn GetAnalyzeType(&self) -> &'static str {
        if self.IndexIDs.is_empty() {
            ANALYZE_STATIC_PARTITION
        } else {
            ANALYZE_STATIC_PARTITION_INDEX
        }
    }
    /// 生成 `ANALYZE TABLE ... PARTITION` SQL 与参数。
    pub fn GenSQLForAnalyzeStaticPartition(&self) -> (String, Vec<String>) {
        (
            "analyze table %n.%n partition %n".to_owned(),
            vec![
                self.SchemaName.clone(),
                self.GlobalTableName.clone(),
                self.StaticPartitionName.clone(),
            ],
        )
    }
    /// 生成 `ANALYZE TABLE ... PARTITION ... INDEX` SQL 与参数。
    pub fn GenSQLForAnalyzeStaticPartitionIndex(&self, index: &str) -> (String, Vec<String>) {
        (
            "analyze table %n.%n partition %n index %n".to_owned(),
            vec![
                self.SchemaName.clone(),
                self.GlobalTableName.clone(),
                self.StaticPartitionName.clone(),
                index.to_owned(),
            ],
        )
    }
    /// 执行单分区 ANALYZE。
    pub fn AnalyzeStaticPartition(&self, runtime: &dyn AnalysisRuntime) -> Result<bool, String> {
        let (sql, params) = self.GenSQLForAnalyzeStaticPartition();
        runtime.execute_analyze(
            &sql,
            &params,
            self.TableStatsVer,
            self.NeedVersionRewriteWarn,
        )
    }
    /// 分析分区上的首个索引；Analyze V2 会同时刷新列及其它索引。
    pub fn AnalyzeStaticPartitionIndexes(
        &self,
        runtime: &dyn AnalysisRuntime,
    ) -> Result<bool, String> {
        if let Some(index) = self.IndexNames.first() {
            let (sql, params) = self.GenSQLForAnalyzeStaticPartitionIndex(index);
            return runtime.execute_analyze(
                &sql,
                &params,
                self.TableStatsVer,
                self.NeedVersionRewriteWarn,
            );
        }
        Ok(true)
    }
    /// 钩子使用 StaticPartitionID（分区级）而非全局表 ID。
    fn finish(&mut self, success: bool) {
        if success {
            if let Some(hook) = self.success_hook.clone() {
                hook(self);
            }
        } else if let Some(hook) = self.failure_hook.clone() {
            hook(self, true);
        }
    }
}

impl AnalysisJob for StaticPartitionedTableAnalysisJob {
    /// 解析全局表与目标分区元数据，并做失败冷却校验（仅本分区）。
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
        let Some(partition) = metadata
            .partitions
            .iter()
            .find(|partition| partition.id == self.StaticPartitionID)
        else {
            if let Some(hook) = self.failure_hook.clone() {
                hook(self, false);
            }
            return (false, PARTITION_NOT_EXIST.to_owned());
        };
        self.SchemaName = metadata.schema_name.clone();
        self.GlobalTableName = metadata.table_name.clone();
        self.StaticPartitionName = partition.name.clone();
        self.IndexNames = index_names(&metadata, &self.IndexIDs);
        let result = IsValidToAnalyze(
            runtime,
            &self.SchemaName,
            &self.GlobalTableName,
            std::slice::from_ref(&self.StaticPartitionName),
        );
        if !result.0
            && let Some(hook) = self.failure_hook.clone()
        {
            hook(self, true);
        }
        result
    }

    /// 按 IndexNames 选择分区整表或分区索引分析。
    fn Analyze(&mut self, runtime: &dyn AnalysisRuntime) -> Result<(), String> {
        let result = match self.GetAnalyzeType() {
            ANALYZE_STATIC_PARTITION => self.AnalyzeStaticPartition(runtime),
            ANALYZE_STATIC_PARTITION_INDEX => self.AnalyzeStaticPartitionIndexes(runtime),
            _ => unreachable!(),
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
    fn HasNewlyAddedIndex(&self) -> bool {
        !self.IndexIDs.is_empty()
    }
    fn GetIndicators(&self) -> Indicators {
        self.indicators.clone()
    }
    fn SetIndicators(&mut self, indicators: Indicators) {
        self.indicators = indicators;
    }
    /// 队列键为分区 ID，使同表不同分区可并行调度。
    fn GetTableID(&self) -> i64 {
        self.StaticPartitionID
    }
    fn RegisterSuccessHook(&mut self, hook: SuccessJobHook) {
        self.success_hook = Some(hook);
    }
    fn RegisterFailureHook(&mut self, hook: FailureJobHook) {
        self.failure_hook = Some(hook);
    }
    fn AsJSON(&self) -> AnalysisJobJSON {
        AnalysisJobJSON {
            Type: self.GetAnalyzeType().to_owned(),
            TableID: self.StaticPartitionID,
            SchemaName: self.SchemaName.clone(),
            TableName: self.GlobalTableName.clone(),
            PartitionNames: vec![self.StaticPartitionName.clone()],
            IndexNames: self.IndexNames.clone(),
            Indicators: AsJSONIndicators(&self.indicators),
            Weight: format!("{:.6}", self.Weight),
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl fmt::Display for StaticPartitionedTableAnalysisJob {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "StaticPartitionedTableAnalysisJob:\n\tAnalyzeType: {}\n\tIndexes: {}\n\tSchema: {}\n\tGlobalTable: {}\n\tGlobalTableID: {}\n\tStaticPartition: {}\n\tStaticPartitionID: {}\n\tTableStatsVer: {}\n\tChangePercentage: {:.6}\n\tTableSize: {:.2}\n\tLastAnalysisDuration: {}\n\tWeight: {:.6}\n",
            self.GetAnalyzeType(),
            self.IndexNames.join(", "),
            self.SchemaName,
            self.GlobalTableName,
            self.GlobalTableID,
            self.StaticPartitionName,
            self.StaticPartitionID,
            self.TableStatsVer,
            self.indicators.ChangePercentage,
            self.indicators.TableSize,
            crate::job::format_go_duration(self.indicators.LastAnalysisDuration),
            self.Weight
        )
    }
}
