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

// 自动 ANALYZE 优先队列作业抽象与公共辅助。
//
// 定义 AnalysisJob / AnalysisRuntime、调度指标 Indicators、失败冷却判定
// IsValidToAnalyze，以及与 Go 格式对齐的 Duration / map 字符串化工具。

use astersql_statistics_handle_logutil::log::{LogField, LogLevel};
use astersql_statistics_handle_logutil::{StatsErrVerboseSampleLogger, StatsSampleLogger};
use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// 无平均耗时记录时，失败后默认等待再分析的时间（30 分钟）。
pub const DEFAULT_FAILED_ANALYSIS_WAIT_TIME: Duration = Duration::from_secs(30 * 60);
/// schema 不存在时的错误文案。
pub const SCHEMA_NOT_EXIST: &str = "schema does not exist";
/// 表不存在时的错误文案。
pub const TABLE_NOT_EXIST: &str = "table does not exist";
/// 表不是分区表时的错误文案。
pub const NOT_PARTITIONED_TABLE: &str = "table is not a partitioned table";
/// 分区不存在时的错误文案。
pub const PARTITION_NOT_EXIST: &str = "partition does not exist";

/// Signed nanoseconds, matching Go time.Duration (including arithmetic overflow).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct AnalysisDuration(i64);

impl AnalysisDuration {
    pub const ZERO: Self = Self(0);
    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }
    pub const fn from_secs(seconds: i64) -> Self {
        Self(seconds.wrapping_mul(1_000_000_000))
    }
    pub const fn as_nanos(self) -> i64 {
        self.0
    }
    pub fn as_secs_f64(self) -> f64 {
        // Go Seconds splits whole and fractional seconds before converting.
        (self.0 / 1_000_000_000) as f64 + (self.0 % 1_000_000_000) as f64 / 1e9
    }
}
impl From<Duration> for AnalysisDuration {
    fn from(duration: Duration) -> Self {
        Self(duration.as_nanos() as i64)
    }
}
impl PartialEq<Duration> for AnalysisDuration {
    fn eq(&self, other: &Duration) -> bool {
        self.0 >= 0 && self.0 as u128 == other.as_nanos()
    }
}
impl PartialEq<AnalysisDuration> for Duration {
    fn eq(&self, other: &AnalysisDuration) -> bool {
        other == self
    }
}
impl fmt::Display for AnalysisDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_go_duration(*self))
    }
}

/// 计算作业权重所用的调度指标：变更比例、表大小、距上次分析时长。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Indicators {
    /// 相对上次分析的数据变更比例（0~1）。
    pub ChangePercentage: f64,
    /// 表规模（行数或字节量级，供权重公式使用）。
    pub TableSize: f64,
    /// 距上次分析的时长。
    pub LastAnalysisDuration: AnalysisDuration,
}

/// 索引元数据，用于解析作业中的索引名/ID。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexMetadata {
    pub id: i64,
    pub name: String,
    /// 是否已 public（对查询可见）。
    pub public: bool,
    /// 是否为列存（columnar）索引。
    pub columnar: bool,
}

/// 分区元数据。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartitionMetadata {
    pub id: i64,
    pub name: String,
}

/// 表元数据：schema/表名、索引与分区列表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableMetadata {
    pub id: i64,
    pub schema_name: String,
    pub table_name: String,
    pub indices: Vec<IndexMetadata>,
    pub partitions: Vec<PartitionMetadata>,
}

/// Indicators 的 JSON/日志友好字符串形式。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndicatorsJSON {
    pub ChangePercentage: String,
    pub TableSize: String,
    pub LastAnalysisDuration: String,
}

/// 分析作业序列化视图，便于日志与调试输出。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnalysisJobJSON {
    pub Type: String,
    pub TableID: i64,
    pub SchemaName: String,
    pub TableName: String,
    pub PartitionNames: Vec<String>,
    pub IndexNames: Vec<String>,
    pub Indicators: IndicatorsJSON,
    pub Weight: String,
}

/// Runtime boundary used by all jobs. Implementations connect the queue to the
/// session/statistics executor without weakening the job state machine.
///
/// 所有作业共用的运行时边界：连接优先队列与会话/统计执行器，不削弱作业状态机。
pub trait AnalysisRuntime: Send + Sync {
    /// 按表 ID 获取元数据。
    fn table_by_id(&self, table_id: i64) -> Option<TableMetadata>;
    /// 最近一次失败分析距今的时长。
    fn last_failed_analysis_duration(
        &self,
        schema: &str,
        table: &str,
        partitions: &[String],
    ) -> Result<Option<Duration>, String>;
    /// 最近成功分析的平均耗时。
    fn average_analysis_duration(
        &self,
        schema: &str,
        table: &str,
        partitions: &[String],
    ) -> Result<Option<Duration>, String>;
    /// 动态分区 ANALYZE 每批分区数上限。
    fn partition_batch_size(&self) -> usize {
        128
    }
    /// 执行一条 ANALYZE SQL。
    fn execute_analyze(
        &self,
        sql: &str,
        params: &[String],
        stats_version: i32,
        need_version_rewrite_warn: bool,
    ) -> Result<bool, String>;
}

/// 作业成功完成后的回调，借用当前完整作业。
pub type SuccessJobHook = Arc<dyn Fn(&mut dyn AnalysisJob) + Send + Sync>;
/// 作业失败后的回调（完整作业，是否必须重试）。
pub type FailureJobHook = Arc<dyn Fn(&mut dyn AnalysisJob, bool) + Send + Sync>;

/// 优先队列中的分析作业接口：校验、执行、权重与钩子。
pub trait AnalysisJob: fmt::Display + Send + Sync {
    /// 校验元数据并准备执行；返回 (是否可分析, 失败原因)。
    fn ValidateAndPrepare(&mut self, runtime: &dyn AnalysisRuntime) -> (bool, String);
    /// 执行 ANALYZE。
    fn Analyze(&mut self, runtime: &dyn AnalysisRuntime) -> Result<(), String>;
    fn SetWeight(&mut self, weight: f64);
    fn GetWeight(&self) -> f64;
    /// 是否因新增索引触发分析。
    fn HasNewlyAddedIndex(&self) -> bool;
    fn GetIndicators(&self) -> Indicators;
    fn SetIndicators(&mut self, indicators: Indicators);
    fn GetTableID(&self) -> i64;
    fn RegisterSuccessHook(&mut self, hook: SuccessJobHook);
    fn RegisterFailureHook(&mut self, hook: FailureJobHook);
    fn AsJSON(&self) -> AnalysisJobJSON;
    fn as_any(&self) -> &dyn Any;
    /// 与 Go String() 对齐的展示字符串。
    fn String(&self) -> String {
        self.to_string()
    }
}

/// 根据最近失败间隔与平均耗时判断当前是否允许再次 ANALYZE（失败冷却）。
pub fn IsValidToAnalyze(
    runtime: &dyn AnalysisRuntime,
    schema: &str,
    table: &str,
    partition_names: &[String],
) -> (bool, String) {
    let fields = || {
        vec![
            LogField::String("schema".into(), schema.into()),
            LogField::String("table".into(), table.into()),
            LogField::Strings("partitions".into(), partition_names.to_vec()),
        ]
    };
    let last_failed = match runtime.last_failed_analysis_duration(schema, table, partition_names) {
        Ok(duration) => duration,
        Err(error) => {
            let mut context = fields();
            context.push(LogField::String("error".into(), error.clone()));
            StatsErrVerboseSampleLogger().log(
                LogLevel::Warn,
                "Fail to get last failed analysis duration",
                context,
            );
            return (
                false,
                format!("fail to get last failed analysis duration: {error}"),
            );
        }
    };
    let average = match runtime.average_analysis_duration(schema, table, partition_names) {
        Ok(duration) => duration,
        Err(error) => {
            let mut context = fields();
            context.push(LogField::String("error".into(), error.clone()));
            StatsErrVerboseSampleLogger().log(
                LogLevel::Warn,
                "Fail to get average analysis duration",
                context,
            );
            return (
                false,
                format!("fail to get average analysis duration: {error}"),
            );
        }
    };

    // Runtime history uses nonnegative wall-clock intervals. Convert at this boundary
    // to Go's signed nanoseconds before testing sentinels or comparing cooldowns.
    let last_failed = last_failed
        .map(AnalysisDuration::from)
        .filter(|d| d.as_nanos() != -1);
    let average = average
        .map(AnalysisDuration::from)
        .filter(|d| d.as_nanos() != -1);
    // 刚失败：立即拒绝。
    if last_failed == Some(AnalysisDuration::ZERO) {
        StatsSampleLogger().log(
            LogLevel::Info,
            "Skip analysis because the last analysis just failed",
            fields(),
        );
        return (false, "last analysis just failed".to_owned());
    }
    if let Some(last_failed) = last_failed {
        let duration_fields = || {
            let mut context = fields();
            context.push(LogField::I64(
                "lastFailedAnalysisDuration".into(),
                last_failed.as_nanos(),
            ));
            context.push(LogField::I64(
                "averageAnalysisDuration".into(),
                average.map_or(-1, AnalysisDuration::as_nanos),
            ));
            context
        };
        // 无平均耗时时，失败间隔须达到默认等待时间。
        if average.is_none() && last_failed < DEFAULT_FAILED_ANALYSIS_WAIT_TIME.into() {
            StatsSampleLogger().log(
                LogLevel::Info,
                format!(
                    "Skip analysis because the last failed analysis duration is less than {}",
                    format_go_duration(DEFAULT_FAILED_ANALYSIS_WAIT_TIME)
                ),
                duration_fields(),
            );
            return (
                false,
                format!(
                    "last failed analysis duration is less than {}",
                    format_go_duration(DEFAULT_FAILED_ANALYSIS_WAIT_TIME)
                ),
            );
        }
        // Go time.Duration uses signed int64 nanoseconds, including wrapping multiplication.
        if let Some(average) = average
            && last_failed.as_nanos() < average.as_nanos().wrapping_mul(2)
        {
            StatsSampleLogger().log(LogLevel::Info, "Skip analysis because the last failed analysis duration is less than 2 times the average analysis duration", duration_fields());
            return (
                false,
                "last failed analysis duration is less than 2 times the average analysis duration"
                    .to_owned(),
            );
        }
    }
    (true, String::new())
}

/// 判断作业是否为动态分区表分析作业类型。
pub fn IsDynamicPartitionedTableAnalysisJob(job: &dyn AnalysisJob) -> bool {
    job.as_any()
        .is::<crate::dynamic_partitioned_table_analysis_job::DynamicPartitionedTableAnalysisJob>()
}

/// Formats a `Duration` the way Go's `time.Duration` `%v`/`String()` does.
///
/// 按 Go `time.Duration` 的 `%v`/`String()` 规则格式化时长。
pub(crate) fn format_go_duration(duration: impl Into<AnalysisDuration>) -> String {
    let nanos = duration.into().as_nanos();
    let text = format_go_duration_magnitude(nanos.unsigned_abs() as u128);
    if nanos < 0 { format!("-{text}") } else { text }
}

fn format_go_duration_magnitude(nanos: u128) -> String {
    if nanos == 0 {
        return "0s".to_owned();
    }
    const MICROSECOND: u128 = 1_000;
    const MILLISECOND: u128 = 1_000 * MICROSECOND;
    const SECOND: u128 = 1_000 * MILLISECOND;
    const MINUTE: u128 = 60 * SECOND;
    const HOUR: u128 = 60 * MINUTE;
    /// 将 value 按 unit 拆成整数与小数部分并拼接单位后缀。
    fn decimal(value: u128, unit: u128, fractional_digits: usize, suffix: &str) -> String {
        let whole = value / unit;
        let remainder = value % unit;
        if remainder == 0 {
            return format!("{whole}{suffix}");
        }
        let scale = 10_u128.pow(fractional_digits as u32);
        let mut fraction = format!("{:0fractional_digits$}", remainder * scale / unit);
        // 去掉尾随 0，贴近 Go 的小数展示。
        while fraction.ends_with('0') {
            fraction.pop();
        }
        format!("{whole}.{fraction}{suffix}")
    }
    if nanos < MICROSECOND {
        return format!("{nanos}ns");
    }
    if nanos < MILLISECOND {
        return decimal(nanos, MICROSECOND, 3, "\u{b5}s");
    }
    if nanos < SECOND {
        return decimal(nanos, MILLISECOND, 6, "ms");
    }
    let hours = nanos / HOUR;
    let after_hours = nanos % HOUR;
    let minutes = after_hours / MINUTE;
    let after_minutes = after_hours % MINUTE;
    let seconds = decimal(after_minutes, SECOND, 9, "s");
    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}")
    } else {
        seconds
    }
}

/// Formats a map the way Go's `%v` prints `map[string][]string`: sorted keys,
/// e.g. `map[idx:[p0 p1]]`.
///
/// 按 Go `%v` 打印 `map[string][]string`：键排序，如 `map[idx:[p0 p1]]`。
pub(crate) fn format_go_string_list_map(map: &HashMap<String, Vec<String>>) -> String {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let entries: Vec<String> = keys
        .into_iter()
        .map(|key| format!("{key}:[{}]", map[key].join(" ")))
        .collect();
    format!("map[{}]", entries.join(" "))
}

// Go fmt uses capitalized infinities with an explicit positive sign.
fn format_go_fixed(value: f64) -> String {
    if value == f64::INFINITY {
        "+Inf".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-Inf".to_owned()
    } else {
        format!("{value:.2}")
    }
}

/// 将 Indicators 转为带百分号与 Go 风格时长的 JSON 视图。
pub fn AsJSONIndicators(indicators: &Indicators) -> IndicatorsJSON {
    IndicatorsJSON {
        ChangePercentage: format!("{}%", format_go_fixed(indicators.ChangePercentage * 100.0)),
        TableSize: format_go_fixed(indicators.TableSize),
        LastAnalysisDuration: format_go_duration(indicators.LastAnalysisDuration),
    }
}

/// 按给定索引 ID 集合从元数据中提取索引名列表。
pub(crate) fn index_names(metadata: &TableMetadata, ids: &HashMap<i64, ()>) -> Vec<String> {
    metadata
        .indices
        .iter()
        .filter(|index| ids.contains_key(&index.id))
        .map(|index| index.name.clone())
        .collect()
}
