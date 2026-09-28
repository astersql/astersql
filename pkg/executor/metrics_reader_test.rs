// Copyright 2026 AsterSQL.

// metrics_reader 单元测试：mock 后端下的单次取数与 skip_request 行为。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::metrics_reader::{
    Datum, MetricRetriever, MetricSummaryTableExtractor, MetricTableDef, MetricTableExtractor,
    MetricsReaderBackend, MetricsSummaryByLabelRetriever, MetricsSummaryRetriever,
    PromQLQueryRange, PrometheusAddress, PrometheusQueryError, PrometheusValue, QueryTimeRange,
    RestrictedRow, SamplePair, SampleStream,
};

/// 仅通过 `mock_table_data` 返回行，真实 Prometheus 路径一律失败。
struct MockMetricsBackend;

impl MetricsReaderBackend for MockMetricsBackend {
    type Context = ();
    type Error = String;
    type PrometheusClient = ();
    type QueryContext = ();

    fn error(&self, message: String) -> Self::Error {
        message
    }
    fn mock_table_data(&self, _: &(), table: &str) -> Option<Vec<Vec<Datum>>> {
        (table == "tidb_query_duration").then(|| {
            vec![vec![
                Datum::String("instance-1".into()),
                Datum::Float64(0.99),
            ]]
        })
    }
    fn mock_prometheus_data(&self, _: &()) -> Option<PrometheusValue> {
        None
    }
    fn metric_table_def(&self, _: &str) -> Result<MetricTableDef, String> {
        Err("mock rows must bypass metric definition".into())
    }
    fn metric_schema_step_seconds(&self) -> i64 {
        60
    }
    fn metric_schema_range_duration(&self) -> i64 {
        60
    }
    fn label_condition_values(&self, _: &[String]) -> String {
        String::new()
    }
    fn generate_promql(
        &self,
        _: &MetricTableDef,
        _: i64,
        _: &HashMap<String, Vec<String>>,
        _: f64,
    ) -> String {
        String::new()
    }
    fn prometheus_address(&self) -> PrometheusAddress<String> {
        PrometheusAddress::NotSet("not configured".into())
    }
    fn sleep(&self, _: Duration) {}
    fn new_prometheus_client(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn query_context(&self, _: &(), _: Duration) {}
    fn query_range(
        &self,
        _: &(),
        _: &(),
        _: &str,
        _: &PromQLQueryRange,
    ) -> Result<PrometheusValue, PrometheusQueryError<String>> {
        Err(PrometheusQueryError::Other("unexpected query".into()))
    }
    fn has_process_privilege(&self) -> bool {
        true
    }
    fn process_access_denied(&self) -> String {
        "denied".into()
    }
    fn metric_table_names(&self) -> Vec<String> {
        vec!["tidb_query_duration".into()]
    }
    fn append_warning(&self, _: String) {}
    fn restricted_sql(&self, _: &str) -> Result<Vec<RestrictedRow>, String> {
        Ok(Vec::new())
    }
}

/// 构造指向 `tidb_query_duration` 的 MetricRetriever。
fn retriever(skip_request: bool) -> MetricRetriever {
    MetricRetriever {
        table_name: "tidb_query_duration".into(),
        tblDef: None,
        extractor: MetricTableExtractor {
            skip_request,
            quantiles: vec![0.99],
            start_time: SystemTime::UNIX_EPOCH,
            end_time: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            label_conditions: HashMap::new(),
        },
        retrieved: false,
    }
}

/// 首次 retrieve 走 mock 行；再次为空；skip_request 时不标记 retrieved。
#[test]
fn metric_retriever_uses_mock_rows_once_and_honors_skip_request() {
    let backend = MockMetricsBackend;
    let mut reader = retriever(false);
    assert_eq!(
        reader.retrieve(&(), &backend).unwrap(),
        vec![vec![
            Datum::String("instance-1".into()),
            Datum::Float64(0.99)
        ]]
    );
    assert!(reader.retrieve(&(), &backend).unwrap().is_empty());

    let mut skipped = retriever(true);
    assert!(skipped.retrieve(&(), &backend).unwrap().is_empty());
    assert!(!skipped.retrieved);
}

struct ParityBackend {
    address_attempts: Cell<usize>,
    query_attempts: Cell<usize>,
    sleeps: Cell<usize>,
    timeout: Cell<Option<Duration>>,
    sql: RefCell<Vec<String>>,
    warnings: RefCell<Vec<String>>,
    process_privilege: bool,
}

impl ParityBackend {
    fn new(process_privilege: bool) -> Self {
        Self {
            address_attempts: Cell::new(0),
            query_attempts: Cell::new(0),
            sleeps: Cell::new(0),
            timeout: Cell::new(None),
            sql: RefCell::new(Vec::new()),
            warnings: RefCell::new(Vec::new()),
            process_privilege,
        }
    }
}

impl MetricsReaderBackend for ParityBackend {
    type Context = ();
    type Error = String;
    type PrometheusClient = ();
    type QueryContext = ();

    fn error(&self, message: String) -> String {
        message
    }
    fn mock_table_data(&self, _: &(), _: &str) -> Option<Vec<Vec<Datum>>> {
        None
    }
    fn mock_prometheus_data(&self, _: &()) -> Option<PrometheusValue> {
        None
    }
    fn metric_table_def(&self, table: &str) -> Result<MetricTableDef, String> {
        match table {
            "missing" => Err("missing".into()),
            "plain" => Ok(MetricTableDef {
                labels: Vec::new(),
                quantile: 0.0,
                comment: "plain metric".into(),
            }),
            _ => Ok(MetricTableDef {
                labels: vec!["instance".into(), "store".into()],
                quantile: 0.99,
                comment: "latency".into(),
            }),
        }
    }
    fn metric_schema_step_seconds(&self) -> i64 {
        15
    }
    fn metric_schema_range_duration(&self) -> i64 {
        60
    }
    fn label_condition_values(&self, values: &[String]) -> String {
        values.join("|")
    }
    fn generate_promql(
        &self,
        _: &MetricTableDef,
        range_duration: i64,
        conditions: &HashMap<String, Vec<String>>,
        quantile: f64,
    ) -> String {
        format!(
            "range={range_duration},store={},q={quantile}",
            conditions["store"].join("|")
        )
    }
    fn prometheus_address(&self) -> PrometheusAddress<String> {
        let attempt = self.address_attempts.get() + 1;
        self.address_attempts.set(attempt);
        if attempt < 3 {
            PrometheusAddress::Error("temporary address error".into())
        } else {
            PrometheusAddress::Address("http://prometheus".into())
        }
    }
    fn sleep(&self, duration: Duration) {
        assert_eq!(duration, Duration::from_millis(100));
        self.sleeps.set(self.sleeps.get() + 1);
    }
    fn new_prometheus_client(&self, address: &str) -> Result<(), String> {
        assert_eq!(address, "http://prometheus");
        Ok(())
    }
    fn query_context(&self, _: &(), timeout: Duration) {
        self.timeout.set(Some(timeout));
    }
    fn query_range(
        &self,
        _: &(),
        _: &(),
        promql: &str,
        range: &PromQLQueryRange,
    ) -> Result<PrometheusValue, PrometheusQueryError<String>> {
        assert_eq!(promql, "range=60,store=7|8,q=0.9");
        assert_eq!(range.step_seconds, 15);
        let attempt = self.query_attempts.get() + 1;
        self.query_attempts.set(attempt);
        if attempt < 3 {
            return Err(PrometheusQueryError::Other("temporary query error".into()));
        }
        Ok(PrometheusValue::Matrix(vec![SampleStream {
            metric: HashMap::from([("instance".into(), "node-1".into())]),
            values: vec![
                SamplePair {
                    timestamp_millis: 1_000,
                    value: 3.5,
                },
                SamplePair {
                    timestamp_millis: 2_000,
                    value: f64::NAN,
                },
            ],
        }]))
    }
    fn has_process_privilege(&self) -> bool {
        self.process_privilege
    }
    fn process_access_denied(&self) -> String {
        "PROCESS denied".into()
    }
    fn metric_table_names(&self) -> Vec<String> {
        vec!["plain".into(), "missing".into(), "latency".into()]
    }
    fn append_warning(&self, message: String) {
        self.warnings.borrow_mut().push(message);
    }
    fn restricted_sql(&self, sql: &str) -> Result<Vec<RestrictedRow>, String> {
        self.sql.borrow_mut().push(sql.into());
        if sql.contains("`latency`") && sql.contains("group by") {
            Ok(vec![RestrictedRow {
                values: vec![
                    Datum::Float64(10.0),
                    Datum::Float64(5.0),
                    Datum::Float64(2.0),
                    Datum::Float64(8.0),
                    Datum::String("node-1".into()),
                    Datum::String("7".into()),
                    Datum::Float64(0.9),
                ],
            }])
        } else if sql.contains("`latency`") {
            Ok(vec![RestrictedRow {
                values: vec![
                    Datum::Float64(10.0),
                    Datum::Float64(5.0),
                    Datum::Float64(2.0),
                    Datum::Float64(8.0),
                    Datum::Float64(0.9),
                ],
            }])
        } else {
            Ok(vec![RestrictedRow {
                values: vec![
                    Datum::Float64(4.0),
                    Datum::Float64(2.0),
                    Datum::Float64(1.0),
                    Datum::Float64(3.0),
                ],
            }])
        }
    }
}

#[test]
fn metric_retriever_retries_and_matches_go_record_shape() {
    let backend = ParityBackend::new(true);
    let mut reader = MetricRetriever {
        table_name: "latency".into(),
        tblDef: None,
        extractor: MetricTableExtractor {
            skip_request: false,
            quantiles: vec![0.9],
            start_time: SystemTime::UNIX_EPOCH,
            end_time: SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            label_conditions: HashMap::from([("store".into(), vec!["7".into(), "8".into()])]),
        },
        retrieved: false,
    };

    assert_eq!(
        reader.retrieve(&(), &backend).unwrap(),
        vec![
            vec![
                Datum::TimeMillis(1_000),
                Datum::String("node-1".into()),
                Datum::String("7|8".into()),
                Datum::Float64(0.9),
                Datum::Float64(3.5)
            ],
            vec![
                Datum::TimeMillis(2_000),
                Datum::String("node-1".into()),
                Datum::String("7|8".into()),
                Datum::Float64(0.9),
                Datum::Null
            ],
        ]
    );
    assert_eq!(backend.address_attempts.get(), 3);
    assert_eq!(backend.query_attempts.get(), 3);
    assert_eq!(backend.sleeps.get(), 4);
    assert_eq!(backend.timeout.get(), Some(Duration::from_secs(10)));
}

#[test]
fn metrics_summaries_match_go_sql_rows_warnings_and_privilege_order() {
    let denied = ParityBackend::new(false);
    let mut denied_reader = MetricsSummaryRetriever {
        extractor: MetricSummaryTableExtractor {
            skip_request: true,
            ..Default::default()
        },
        timeRange: QueryTimeRange {
            condition: "where time > 0".into(),
        },
        retrieved: false,
    };
    assert_eq!(
        denied_reader.retrieve(&(), &denied).unwrap_err(),
        "PROCESS denied"
    );

    let backend = ParityBackend::new(true);
    let extractor = MetricSummaryTableExtractor {
        skip_request: false,
        metrics_names: ["latency".into(), "missing".into()].into_iter().collect(),
        quantiles: vec![0.9],
    };
    let mut summary = MetricsSummaryRetriever {
        extractor: extractor.clone(),
        timeRange: QueryTimeRange {
            condition: "where time > 0".into(),
        },
        retrieved: false,
    };
    assert_eq!(
        summary.retrieve(&(), &backend).unwrap(),
        vec![vec![
            Datum::String("latency".into()),
            Datum::Float64(0.9),
            Datum::Float64(10.0),
            Datum::Float64(5.0),
            Datum::Float64(2.0),
            Datum::Float64(8.0),
            Datum::String("latency".into()),
        ]]
    );
    assert_eq!(
        backend.warnings.borrow().as_slice(),
        ["metrics table: missing not found"]
    );
    assert!(
        backend.sql.borrow()[0]
            .contains("quantile in (0.900000) group by quantile order by quantile")
    );

    let mut by_label = MetricsSummaryByLabelRetriever {
        extractor,
        timeRange: QueryTimeRange {
            condition: "where time > 0".into(),
        },
        retrieved: false,
    };
    assert_eq!(
        by_label.retrieve(&(), &backend).unwrap(),
        vec![vec![
            Datum::String("node-1".into()),
            Datum::String("latency".into()),
            Datum::String("store_id:7".into()),
            Datum::Float64(0.9),
            Datum::Float64(10.0),
            Datum::Float64(5.0),
            Datum::Float64(2.0),
            Datum::Float64(8.0),
            Datum::String("latency".into()),
        ]]
    );
    assert!(
        backend.sql.borrow()[1].contains(
            "group by `instance`,`store`,`quantile` order by `instance`,`store`,`quantile`"
        )
    );
}
