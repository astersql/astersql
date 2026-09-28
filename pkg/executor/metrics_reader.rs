// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `METRICS_SCHEMA` 表读取：把 Prometheus 查询结果转成 SQL 行，并汇总摘要表。
//
// 通过 `MetricsReaderBackend` 注入 InfoSync、权限、受限 SQL 与 PromQL 查询；
// 支持单表检索、按表汇总、按 label（标签）汇总三类 Retriever。

#![allow(non_snake_case, non_upper_case_globals)]

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::time::{Duration, SystemTime};

/// 单次 Prometheus range/query_range 的超时时间。
pub const promReadTimeout: Duration = Duration::from_secs(10);

/// 受限 SQL / 指标行中的单元格值。
#[derive(Clone, Debug, PartialEq)]
pub enum Datum {
    Null,
    TimeMillis(i64),
    String(String),
    Float64(f64),
}

/// Prometheus 时间序列上的一个采样点（毫秒时间戳 + 值）。
#[derive(Clone, Debug, PartialEq)]
pub struct SamplePair {
    pub timestamp_millis: i64,
    pub value: f64,
}

/// 带 metric 标签的采样流（对应 Prometheus matrix 中的一条序列）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SampleStream {
    pub metric: HashMap<String, String>,
    pub values: Vec<SamplePair>,
}

/// Prometheus 查询返回值；本路径只消费 Matrix。
#[derive(Clone, Debug, PartialEq)]
pub enum PrometheusValue {
    Matrix(Vec<SampleStream>),
    Other,
}

/// `metrics_schema` 中一张逻辑指标表的定义。
#[derive(Clone, Debug, PartialEq)]
pub struct MetricTableDef {
    /// 输出列对应的 Prometheus label 名。
    pub labels: Vec<String>,
    /// 默认分位数；>0 表示直方图类指标需要 quantile 列。
    pub quantile: f64,
    /// 表注释，摘要行会带回。
    pub comment: String,
}

/// 从谓词提取的查询参数：时间窗、分位数与 label 过滤条件。
#[derive(Clone, Debug)]
pub struct MetricTableExtractor {
    /// 优化器判定无需真正拉数时置 true。
    pub skip_request: bool,
    pub quantiles: Vec<f64>,
    pub start_time: SystemTime,
    pub end_time: SystemTime,
    pub label_conditions: HashMap<String, Vec<String>>,
}

/// 摘要表（metrics_summary / by_label）共用的提取结果。
#[derive(Clone, Debug, Default)]
pub struct MetricSummaryTableExtractor {
    pub skip_request: bool,
    /// 非空时只保留列出的指标表名。
    pub metrics_names: HashSet<String>,
    pub quantiles: Vec<f64>,
}

/// 摘要 SQL 中附加的时间范围 WHERE 片段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryTimeRange {
    pub condition: String,
}

/// PromQL `query_range` 的起止时间与步长（秒）。
#[derive(Clone, Debug)]
pub struct PromQLQueryRange {
    pub start: SystemTime,
    pub end: SystemTime,
    pub step_seconds: i64,
}

/// 受限 SQL 返回的一行。
#[derive(Clone, Debug, PartialEq)]
pub struct RestrictedRow {
    pub values: Vec<Datum>,
}

impl RestrictedRow {
    /// 按列下标取字符串；类型不符则 panic（与 Go 强转语义对齐）。
    fn string(&self, index: usize) -> &str {
        match &self.values[index] {
            Datum::String(value) => value,
            _ => panic!("restricted SQL column {index} must be a string"),
        }
    }

    /// 按列下标取浮点。
    fn float64(&self, index: usize) -> f64 {
        match self.values[index] {
            Datum::Float64(value) => value,
            _ => panic!("restricted SQL column {index} must be a float"),
        }
    }
}

/// Prometheus 地址解析结果：已配置 / 未配置 / 瞬时错误。
pub enum PrometheusAddress<E> {
    Address(String),
    NotSet(E),
    Error(E),
}

/// Prometheus 查询失败：API 业务错误或其它后端错误。
pub enum PrometheusQueryError<E> {
    Api { message: String, detail: String },
    Other(E),
}

/// Production boundary for InfoSync, Prometheus, session privilege, warnings,
/// failpoints, and restricted SQL. There are no successful fallback methods.
///
/// 生产边界：InfoSync、Prometheus、会话权限、警告、failpoint 与受限 SQL；
/// 无“默认成功”的回退实现。
pub trait MetricsReaderBackend {
    type Context: Clone;
    type Error: Display;
    type PrometheusClient;
    type QueryContext;

    fn error(&self, message: String) -> Self::Error;
    fn mock_table_data(&self, context: &Self::Context, table_name: &str)
    -> Option<Vec<Vec<Datum>>>;
    fn mock_prometheus_data(&self, context: &Self::Context) -> Option<PrometheusValue>;
    fn metric_table_def(&self, table_name: &str) -> Result<MetricTableDef, Self::Error>;
    fn metric_schema_step_seconds(&self) -> i64;
    fn metric_schema_range_duration(&self) -> i64;
    fn label_condition_values(&self, conditions: &[String]) -> String;
    fn generate_promql(
        &self,
        table_def: &MetricTableDef,
        range_duration: i64,
        label_conditions: &HashMap<String, Vec<String>>,
        quantile: f64,
    ) -> String;
    fn prometheus_address(&self) -> PrometheusAddress<Self::Error>;
    fn sleep(&self, duration: Duration);
    fn new_prometheus_client(&self, address: &str) -> Result<Self::PrometheusClient, Self::Error>;
    fn query_context(&self, context: &Self::Context, timeout: Duration) -> Self::QueryContext;
    fn query_range(
        &self,
        context: &Self::QueryContext,
        client: &Self::PrometheusClient,
        promql: &str,
        query_range: &PromQLQueryRange,
    ) -> Result<PrometheusValue, PrometheusQueryError<Self::Error>>;

    fn has_process_privilege(&self) -> bool;
    fn process_access_denied(&self) -> Self::Error;
    fn metric_table_names(&self) -> Vec<String>;
    fn append_warning(&self, message: String);
    fn restricted_sql(&self, sql: &str) -> Result<Vec<RestrictedRow>, Self::Error>;
}

/// 单张 metrics 表的 Retriever：按分位数查询并生成行。
pub struct MetricRetriever {
    pub table_name: String,
    pub tblDef: Option<MetricTableDef>,
    pub extractor: MetricTableExtractor,
    /// 是否已拉取过（保证只请求一次）。
    pub retrieved: bool,
}

impl MetricRetriever {
    /// 拉取指标行；已取过或 skip_request 时返回空。
    pub fn retrieve<B: MetricsReaderBackend>(
        &mut self,
        context: &B::Context,
        backend: &B,
    ) -> Result<Vec<Vec<Datum>>, B::Error> {
        if self.retrieved || self.extractor.skip_request {
            return Ok(Vec::new());
        }
        self.retrieved = true;
        // Failpoint / 测试可直接注入表数据，跳过 Prometheus。
        if let Some(rows) = backend.mock_table_data(context, &self.table_name) {
            return Ok(rows);
        }

        let table_def = backend.metric_table_def(&self.table_name)?;
        self.tblDef = Some(table_def.clone());
        let query_range = self.getQueryRange(backend);
        // 未指定分位数时使用表定义默认值。
        let quantiles = if self.extractor.quantiles.is_empty() {
            vec![table_def.quantile]
        } else {
            self.extractor.quantiles.clone()
        };
        let mut total_rows = Vec::new();
        for quantile in quantiles {
            let query_value = match self.queryMetric(context, backend, &query_range, quantile) {
                Ok(value) => value,
                Err(PrometheusQueryError::Api { message, detail }) => {
                    return Err(backend.error(format!(
                        "query metric error, msg: {message}, detail: {detail}"
                    )));
                }
                Err(PrometheusQueryError::Other(error)) => {
                    return Err(backend.error(format!("query metric error: {error}")));
                }
            };
            total_rows.extend(self.genRows(backend, query_value, quantile));
        }
        Ok(total_rows)
    }

    /// 解析 Prometheus 地址并执行 query_range（带有限次重试）。
    fn queryMetric<B: MetricsReaderBackend>(
        &self,
        context: &B::Context,
        backend: &B,
        query_range: &PromQLQueryRange,
        quantile: f64,
    ) -> Result<PrometheusValue, PrometheusQueryError<B::Error>> {
        if let Some(value) = backend.mock_prometheus_data(context) {
            return Ok(value);
        }

        // 最多 5 次尝试获取地址：Error 可重试，NotSet 立即失败。
        let mut address = String::new();
        let mut address_error = None;
        for _ in 0..5 {
            match backend.prometheus_address() {
                PrometheusAddress::Address(value) => {
                    address = value;
                    address_error = None;
                    break;
                }
                PrometheusAddress::NotSet(error) => {
                    address_error = Some(error);
                    break;
                }
                PrometheusAddress::Error(error) => {
                    address_error = Some(error);
                    backend.sleep(Duration::from_millis(100));
                }
            }
        }
        if let Some(error) = address_error {
            return Err(PrometheusQueryError::Other(error));
        }
        let client = backend
            .new_prometheus_client(&address)
            .map_err(PrometheusQueryError::Other)?;
        let query_context = backend.query_context(context, promReadTimeout);

        let table_def = self.tblDef.as_ref().expect("table definition is loaded");
        let promql = backend.generate_promql(
            table_def,
            backend.metric_schema_range_duration(),
            &self.extractor.label_conditions,
            quantile,
        );
        // 查询本身同样最多重试 5 次。
        let mut result = None;
        for _ in 0..5 {
            match backend.query_range(&query_context, &client, &promql, query_range) {
                Ok(value) => return Ok(value),
                Err(error) => {
                    result = Some(error);
                    backend.sleep(Duration::from_millis(100));
                }
            }
        }
        Err(result.expect("five failed Prometheus attempts record an error"))
    }

    /// 从 extractor 时间窗与 schema 步长构造 query_range。
    fn getQueryRange<B: MetricsReaderBackend>(&self, backend: &B) -> PromQLQueryRange {
        PromQLQueryRange {
            start: self.extractor.start_time,
            end: self.extractor.end_time,
            step_seconds: backend.metric_schema_step_seconds(),
        }
    }

    /// 将 Matrix 结果展开为多行 Datum。
    fn genRows<B: MetricsReaderBackend>(
        &self,
        backend: &B,
        value: PrometheusValue,
        quantile: f64,
    ) -> Vec<Vec<Datum>> {
        let PrometheusValue::Matrix(matrix) = value else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for stream in matrix {
            for pair in stream.values {
                rows.push(self.genRecord(backend, &stream.metric, &pair, quantile));
            }
        }
        rows
    }

    /// 构造单行：时间、labels、可选 quantile、指标值（NaN → Null）。
    fn genRecord<B: MetricsReaderBackend>(
        &self,
        backend: &B,
        metric: &HashMap<String, String>,
        pair: &SamplePair,
        quantile: f64,
    ) -> Vec<Datum> {
        let table_def = self.tblDef.as_ref().expect("table definition is loaded");
        let mut record = Vec::with_capacity(2 + table_def.labels.len() + 1);
        record.push(Datum::TimeMillis(pair.timestamp_millis));
        for label in &table_def.labels {
            let mut value = metric.get(label).cloned().unwrap_or_default();
            // 序列上缺 label 时，用谓词中的条件值填充。
            if value.is_empty() {
                value = backend.label_condition_values(
                    self.extractor
                        .label_conditions
                        .get(&label.to_lowercase())
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                );
            }
            record.push(Datum::String(value));
        }
        if table_def.quantile > 0.0 {
            record.push(Datum::Float64(quantile));
        }
        if pair.value.is_nan() {
            record.push(Datum::Null);
        } else {
            record.push(Datum::Float64(pair.value));
        }
        record
    }
}

/// 测试用 mock Prometheus 数据的键类型占位。
pub struct MockMetricsPromDataKey;

/// `metrics_summary`：对每张指标表做聚合摘要（需 PROCESS 权限）。
pub struct MetricsSummaryRetriever {
    pub extractor: MetricSummaryTableExtractor,
    pub timeRange: QueryTimeRange,
    pub retrieved: bool,
}

impl MetricsSummaryRetriever {
    /// 枚举指标表，经受限 SQL 聚合 sum/avg/min/max（及 quantile）。
    pub fn retrieve<B: MetricsReaderBackend>(
        &mut self,
        _context: &B::Context,
        backend: &B,
    ) -> Result<Vec<Vec<Datum>>, B::Error> {
        if !backend.has_process_privilege() {
            return Err(backend.process_access_denied());
        }
        if self.retrieved || self.extractor.skip_request {
            return Ok(Vec::new());
        }
        self.retrieved = true;
        let mut tables = backend.metric_table_names();
        tables.sort();
        let mut total_rows = Vec::with_capacity(tables.len());
        for name in tables {
            if !metricEnabled(&self.extractor.metrics_names, &name) {
                continue;
            }
            let definition = match backend.metric_table_def(&name) {
                Ok(definition) => definition,
                Err(_) => {
                    backend.append_warning(format!("metrics table: {name} not found"));
                    continue;
                }
            };
            // 直方图表按 quantile 分组；否则只聚合 value。
            let sql = if definition.quantile > 0.0 {
                let quantiles = if self.extractor.quantiles.is_empty() {
                    vec!["0.99".to_owned()]
                } else {
                    self.extractor
                        .quantiles
                        .iter()
                        .map(|value| format!("{value:.6}"))
                        .collect()
                };
                format!(
                    "select sum(value),avg(value),min(value),max(value),quantile from `metrics_schema`.`{name}` {} and quantile in ({}) group by quantile order by quantile",
                    self.timeRange.condition,
                    quantiles.join(",")
                )
            } else {
                format!(
                    "select sum(value),avg(value),min(value),max(value) from `metrics_schema`.`{name}` {}",
                    self.timeRange.condition
                )
            };
            let rows = backend
                .restricted_sql(&sql)
                .map_err(|error| backend.error(format!("execute '{sql}' failed: {error}")))?;
            for row in rows {
                let quantile = if definition.quantile > 0.0 {
                    Datum::Float64(row.float64(row.values.len() - 1))
                } else {
                    Datum::Null
                };
                total_rows.push(vec![
                    Datum::String(name.clone()),
                    quantile,
                    Datum::Float64(row.float64(0)),
                    Datum::Float64(row.float64(1)),
                    Datum::Float64(row.float64(2)),
                    Datum::Float64(row.float64(3)),
                    Datum::String(definition.comment.clone()),
                ]);
            }
        }
        Ok(total_rows)
    }
}

/// `metrics_summary_by_label`：按 label 维度输出摘要行。
pub struct MetricsSummaryByLabelRetriever {
    pub extractor: MetricSummaryTableExtractor,
    pub timeRange: QueryTimeRange,
    pub retrieved: bool,
}

impl MetricsSummaryByLabelRetriever {
    /// 按表定义的 labels（及可选 quantile）分组聚合。
    pub fn retrieve<B: MetricsReaderBackend>(
        &mut self,
        _context: &B::Context,
        backend: &B,
    ) -> Result<Vec<Vec<Datum>>, B::Error> {
        if !backend.has_process_privilege() {
            return Err(backend.process_access_denied());
        }
        if self.retrieved || self.extractor.skip_request {
            return Ok(Vec::new());
        }
        self.retrieved = true;
        let mut tables = backend.metric_table_names();
        tables.sort();
        let mut total_rows = Vec::with_capacity(tables.len());
        for name in tables {
            if !metricEnabled(&self.extractor.metrics_names, &name) {
                continue;
            }
            let definition = match backend.metric_table_def(&name) {
                Ok(definition) => definition,
                Err(_) => {
                    backend.append_warning(format!("metrics table: {name} not found"));
                    continue;
                }
            };
            let mut columns = definition.labels.clone();
            let mut condition = self.timeRange.condition.clone();
            if definition.quantile > 0.0 {
                columns.push("quantile".to_owned());
                if self.extractor.quantiles.is_empty() {
                    condition.push_str(" and quantile=0.99");
                } else {
                    let quantiles: Vec<String> = self
                        .extractor
                        .quantiles
                        .iter()
                        .map(|value| format!("{value:.6}"))
                        .collect();
                    condition.push_str(&format!(" and quantile in ({})", quantiles.join(",")));
                }
            }
            let sql = if columns.is_empty() {
                format!(
                    "select sum(value),avg(value),min(value),max(value) from `metrics_schema`.`{name}` {condition}"
                )
            } else {
                let columns = columns.join("`,`");
                format!(
                    "select sum(value),avg(value),min(value),max(value),`{columns}` from `metrics_schema`.`{name}` {condition} group by `{columns}` order by `{columns}`"
                )
            };
            let rows = backend
                .restricted_sql(&sql)
                .map_err(|error| backend.error(format!("execute '{sql}' failed: {error}")))?;
            // 若首列 label 为 instance，单独提出；其余拼成 labels 字符串。
            let non_instance_label_index = usize::from(
                definition
                    .labels
                    .first()
                    .is_some_and(|label| label == "instance"),
            );
            const SKIP_COLUMNS: usize = 4;
            for row in rows {
                let instance = if non_instance_label_index > 0 {
                    row.string(SKIP_COLUMNS).to_owned()
                } else {
                    String::new()
                };
                let mut labels = Vec::new();
                for (index, label) in definition.labels[non_instance_label_index..]
                    .iter()
                    .enumerate()
                {
                    let mut value = row
                        .string(SKIP_COLUMNS + non_instance_label_index + index)
                        .to_owned();
                    if label == "store" || label == "store_id" {
                        value = format!("store_id:{value}");
                    }
                    labels.push(value);
                }
                let quantile = if definition.quantile > 0.0 {
                    Datum::Float64(row.float64(row.values.len() - 1))
                } else {
                    Datum::Null
                };
                total_rows.push(vec![
                    Datum::String(instance),
                    Datum::String(name.clone()),
                    Datum::String(labels.join(", ")),
                    quantile,
                    Datum::Float64(row.float64(0)),
                    Datum::Float64(row.float64(1)),
                    Datum::Float64(row.float64(2)),
                    Datum::Float64(row.float64(3)),
                    Datum::String(definition.comment.clone()),
                ]);
            }
        }
        Ok(total_rows)
    }
}

/// 空过滤集合表示不过滤；否则仅启用集合中的表名。
fn metricEnabled(filter: &HashSet<String>, name: &str) -> bool {
    filter.is_empty() || filter.contains(name)
}
