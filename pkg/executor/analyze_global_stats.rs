// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 分区表全局统计合并。
//
// 动态分区裁剪（dynamic partition prune）场景下，优化器需要表级全局统计，
// 本模块将各分区上的直方图/TopN/Sketch 合并后持久化，并登记分析作业。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 全局统计（合并各分区统计）过程中的错误。
pub struct GlobalStatsError(pub String);

impl fmt::Display for GlobalStatsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for GlobalStatsError {}

/// 全局统计操作的 Result 别名。
pub type GlobalStatsResult<T = ()> = Result<T, GlobalStatsError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
/// ANALYZE 选项类型：桶数、TopN、样本数、采样率等。
/// TopN：出现频率最高的前 N 个值及其计数。
pub enum analyzeOptionType {
    NumBuckets,
    NumTopN,
    NumSamples,
    SampleRate,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// v2 统计版本下已填充的 ANALYZE 选项映射。
pub struct v2AnalyzeOptions {
    pub filledOptions: BTreeMap<analyzeOptionType, u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
/// 全局统计映射的键：表 ID + 列/索引 ID。
pub struct globalStatsKey {
    pub tableID: i64,
    /// `-1` identifies column statistics; non-negative values identify indexes.
    pub indexID: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 某表/索引需要合并的全局统计元信息。
pub struct globalStatsInfo {
    pub isIndex: i32,
    pub histogramIDs: Vec<i64>,
    pub statsVersion: i32,
}

// globalStatsMap records the partitioned table/index objects that need global stats.
/// 待合并全局统计对象的有序映射。
pub type globalStatsMap = BTreeMap<globalStatsKey, globalStatsInfo>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 可合并的统计组件：直方图、TopN、FM Sketch、CM Sketch。
/// FM/CM Sketch 用于估计 NDV（不同值个数）与近似频次。
pub enum statisticsComponent {
    Histogram,
    TopN,
    FmSketch,
    CmsSketch,
}

/// 合并全局统计时默认处理的全部组件集合。
pub const GLOBAL_STATS_COMPONENTS: [statisticsComponent; 4] = [
    statisticsComponent::Histogram,
    statisticsComponent::TopN,
    statisticsComponent::FmSketch,
    statisticsComponent::CmsSketch,
];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表的可读标识：库名、表名及索引名映射。
pub struct tableIdentity {
    pub databaseName: String,
    pub tableName: String,
    pub indexNames: BTreeMap<i64, String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 全局统计合并作业的展示信息。
pub struct analyzeJob {
    pub databaseName: String,
    pub tableName: String,
    pub jobInfo: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 将各分区统计合并为全局统计并持久化的请求参数。
pub struct mergePartitionStatsRequest {
    pub tableID: i64,
    pub indexID: i64,
    pub info: globalStatsInfo,
    pub options: BTreeMap<analyzeOptionType, u64>,
    pub components: Vec<statisticsComponent>,
}

/// 全局统计合并依赖的运行时：作业登记、合并落盘、历史记录与日志。
pub trait globalStatsRuntime: Send + Sync {
    fn table_identity(&self, table_id: i64) -> Option<tableIdentity>;
    fn add_new_analyze_job(&self, job: &analyzeJob) -> GlobalStatsResult;
    fn start_analyze_job(&self, job: &analyzeJob);
    /// Loads all partition statistics, validates their versions, merges the
    /// requested histogram/TopN/FM/CMS structures, and persists global stats.
    fn merge_partition_stats_to_global_and_persist(
        &self,
        request: mergePartitionStatsRequest,
    ) -> GlobalStatsResult;
    fn finish_global_stats_job(&self, job: &analyzeJob, error: Option<&GlobalStatsError>);
    fn record_historical_stats(&self, table_id: i64) -> GlobalStatsResult;
    fn log_missing_partitioned_table(&self, table_id: i64);
    fn log_add_job_error(&self, job: &analyzeJob, error: &GlobalStatsError);
    fn log_merge_error(&self, job: &analyzeJob, table_id: i64, error: &GlobalStatsError);
    fn log_historical_error(&self, table_id: i64, error: &GlobalStatsError);
}

/// 负责驱动分区表全局统计合并的 ANALYZE 执行侧入口。
pub struct AnalyzeExec {
    pub options: BTreeMap<analyzeOptionType, u64>,
    pub OptionsMap: Option<BTreeMap<i64, v2AnalyzeOptions>>,
    pub runtime: Arc<dyn globalStatsRuntime>,
}

impl AnalyzeExec {
    /// 按表遍历待合并对象，启动作业、合并分区统计并记录历史。
    /// 合并/历史失败仅记诊断，不使本函数整体失败（与 Go 行为一致）。
    pub fn handleGlobalStats(&self, statsMap: globalStatsMap) -> GlobalStatsResult {
        let global_table_ids = statsMap
            .keys()
            .map(|key| key.tableID)
            .collect::<BTreeSet<_>>();
        let mut historical_table_ids = BTreeSet::new();

        for table_id in global_table_ids {
            historical_table_ids.insert(table_id);
            for (key, info) in &statsMap {
                if key.tableID != table_id {
                    continue;
                }
                let Some(job) = self.newAnalyzeHandleGlobalStatsJob(*key) else {
                    self.runtime.log_missing_partitioned_table(key.tableID);
                    continue;
                };
                if let Err(error) = self.runtime.add_new_analyze_job(&job) {
                    self.runtime.log_add_job_error(&job, &error);
                }
                self.runtime.start_analyze_job(&job);

                let options = self
                    .OptionsMap
                    .as_ref()
                    .and_then(|options| options.get(&key.tableID))
                    .map_or_else(
                        || self.options.clone(),
                        |options| options.filledOptions.clone(),
                    );
                let merge_result = self.runtime.merge_partition_stats_to_global_and_persist(
                    mergePartitionStatsRequest {
                        tableID: key.tableID,
                        indexID: key.indexID,
                        info: info.clone(),
                        options,
                        components: GLOBAL_STATS_COMPONENTS.to_vec(),
                    },
                );
                if let Err(error) = &merge_result {
                    self.runtime.log_merge_error(&job, table_id, error);
                }
                self.runtime
                    .finish_global_stats_job(&job, merge_result.as_ref().err());
            }
        }

        // Merge and history failures are job-level diagnostics. As in Go, they
        // do not turn handleGlobalStats itself into a statement error.
        for table_id in historical_table_ids {
            if let Err(error) = self.runtime.record_historical_stats(table_id) {
                self.runtime.log_historical_error(table_id, &error);
            }
        }
        Ok(())
    }

    /// 根据表身份与列/索引键构造「合并全局统计」作业描述。
    pub fn newAnalyzeHandleGlobalStatsJob(&self, key: globalStatsKey) -> Option<analyzeJob> {
        let table = self.runtime.table_identity(key.tableID)?;
        let job_info = if key.indexID == -1 {
            format!(
                "merge global stats for {}.{} columns",
                table.databaseName, table.tableName
            )
        } else {
            let index_name = table
                .indexNames
                .get(&key.indexID)
                .cloned()
                .unwrap_or_default();
            format!(
                "merge global stats for {}.{}'s index {}",
                table.databaseName, table.tableName, index_name
            )
        };
        Some(analyzeJob {
            databaseName: table.databaseName,
            tableName: table.tableName,
            jobInfo: job_info,
        })
    }
}
