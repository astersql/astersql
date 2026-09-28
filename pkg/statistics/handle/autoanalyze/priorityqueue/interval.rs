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

// 分析历史耗时查询与冷却判定辅助。
//
// 从 `mysql.analyze_jobs` 读取最近成功 ANALYZE 的平均耗时、以及最近失败距今的间隔，
// 供优先队列在调度前判断是否允许再次分析（失败冷却）。

use crate::job::DEFAULT_FAILED_ANALYSIS_WAIT_TIME;
use std::time::Duration;

/// 查询无记录时的哨兵值（对应 Go NoRecord）。
pub const NO_RECORD: i64 = -1;
/// 刚失败（间隔为 0 秒）的哨兵值。
pub const JUST_FAILED: i64 = 0;

/// 非分区表：最近 5 条成功 ANALYZE 的平均秒数。
pub const AVG_DURATION_QUERY_FOR_TABLE: &str = r#"
 SELECT AVG(TIMESTAMPDIFF(SECOND, start_time, end_time)) AS avg_duration
 FROM (SELECT start_time, end_time FROM mysql.analyze_jobs
 WHERE table_schema = %? AND table_name = %? AND state = 'finished'
 AND fail_reason IS NULL AND partition_name = '' ORDER BY id DESC LIMIT 5) AS recent_analyses"#;

/// 分区表：指定分区上最近 5 条成功 ANALYZE 的平均秒数。
pub const AVG_DURATION_QUERY_FOR_PARTITION: &str = r#"
 SELECT AVG(TIMESTAMPDIFF(SECOND, start_time, end_time)) AS avg_duration
 FROM (SELECT start_time, end_time FROM mysql.analyze_jobs
 WHERE table_schema = %? AND table_name = %? AND state = 'finished'
 AND fail_reason IS NULL AND partition_name in (%?) ORDER BY id DESC LIMIT 5) AS recent_analyses"#;

/// 非分区表：最近一次失败距当前时间的秒数。
pub const LAST_FAILED_DURATION_QUERY_FOR_TABLE: &str = r#"
 SELECT TIMESTAMPDIFF(SECOND, start_time, CURRENT_TIMESTAMP)
 FROM mysql.analyze_jobs WHERE table_schema = %? AND table_name = %?
 AND state = 'failed' AND partition_name = '' ORDER BY id DESC LIMIT 1"#;

/// 分区表：各分区最近失败中，距今最短的间隔秒数。
pub const LAST_FAILED_DURATION_QUERY_FOR_PARTITION: &str = r#"
 SELECT MIN(TIMESTAMPDIFF(SECOND, aj.start_time, CURRENT_TIMESTAMP)) AS min_duration
 FROM (SELECT MAX(id) AS max_id FROM mysql.analyze_jobs
 WHERE table_schema = %? AND table_name = %? AND state = 'failed'
 AND partition_name IN (%?) GROUP BY partition_name) AS latest_failures
 JOIN mysql.analyze_jobs aj ON aj.id = latest_failures.max_id"#;

/// 读取分析历史的抽象：由会话/测试替身实现可选数值查询。
pub trait AnalysisHistoryReader {
    /// 执行 SQL，返回可选 f64（用于平均耗时）。
    fn query_optional_f64(&self, sql: &str, params: &[String]) -> Result<Option<f64>, String>;
    /// 执行 SQL，返回可选 i64（用于失败间隔秒数）。
    fn query_optional_i64(&self, sql: &str, params: &[String]) -> Result<Option<i64>, String>;
}

/// 按是否带分区名选择表级或分区级 SQL，并组装绑定参数。
fn query_parts<'a>(
    schema: &str,
    table: &str,
    partition_names: &'a [String],
    table_query: &'static str,
    partition_query: &'static str,
) -> (&'static str, Vec<String>) {
    let mut params = vec![schema.to_owned(), table.to_owned()];
    if partition_names.is_empty() {
        (table_query, params)
    } else {
        params.extend_from_slice(partition_names);
        (partition_query, params)
    }
}

/// 查询最近成功 ANALYZE 的平均耗时；无有效记录返回 None。
pub fn GetAverageAnalysisDuration(
    reader: &dyn AnalysisHistoryReader,
    schema: &str,
    table_name: &str,
    partition_names: &[String],
) -> Result<Option<Duration>, String> {
    let (query, params) = query_parts(
        schema,
        table_name,
        partition_names,
        AVG_DURATION_QUERY_FOR_TABLE,
        AVG_DURATION_QUERY_FOR_PARTITION,
    );
    match reader.query_optional_f64(query, &params)? {
        // 向下取整秒数，与 Go 侧对 AVG 结果的处理一致。
        Some(seconds) if seconds >= 0.0 => Ok(Some(Duration::from_secs_f64(seconds.floor()))),
        _ => Ok(None),
    }
}

/// 查询最近失败距今的间隔；刚失败为 0，负间隔回退为默认等待时间。
pub fn GetLastFailedAnalysisDuration(
    reader: &dyn AnalysisHistoryReader,
    schema: &str,
    table_name: &str,
    partition_names: &[String],
) -> Result<Option<Duration>, String> {
    let (query, params) = query_parts(
        schema,
        table_name,
        partition_names,
        LAST_FAILED_DURATION_QUERY_FOR_TABLE,
        LAST_FAILED_DURATION_QUERY_FOR_PARTITION,
    );
    match reader.query_optional_i64(query, &params)? {
        None => Ok(None),
        Some(0) => Ok(Some(Duration::ZERO)),
        // 时钟/时区异常导致负间隔时，使用默认失败冷却时间。
        Some(seconds) if seconds < 0 => Ok(Some(DEFAULT_FAILED_ANALYSIS_WAIT_TIME)),
        Some(seconds) => Ok(Some(Duration::from_secs(seconds as u64))),
    }
}
