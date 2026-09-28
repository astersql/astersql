// Copyright 2026 AsterSQL.

// 巡检摘要（inspection summary）规则目录的单元测试。
//
// 确认摘要规则表非空、无重复指标，并包含关键 query 延迟指标。

use std::collections::{HashMap, HashSet};

use crate::inspection_summary::{
    InspectionSummaryExtractor, InspectionSummaryRetriever, InspectionSummaryRuntime,
    InspectionSummaryValue, MetricDefinition, inspectionSummaryRules,
};

#[derive(Default)]
struct MockRuntime {
    definitions: HashMap<String, MetricDefinition>,
    rows: Vec<Vec<InspectionSummaryValue>>,
    warnings: Vec<String>,
    sql: Vec<String>,
    error: Option<String>,
}

impl InspectionSummaryRuntime for MockRuntime {
    type Context = ();
    type MetricRow = Vec<InspectionSummaryValue>;
    type Error = String;

    fn metric_definition(&self, name: &str) -> Option<MetricDefinition> {
        self.definitions.get(name).cloned()
    }

    fn append_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    fn execute_restricted_sql(
        &mut self,
        _context: &mut Self::Context,
        sql: &str,
    ) -> Result<Vec<Self::MetricRow>, Self::Error> {
        self.sql.push(sql.to_owned());
        if let Some(error) = self.error.clone() {
            Err(error)
        } else {
            Ok(self.rows.clone())
        }
    }

    fn row_len(&self, row: &Self::MetricRow) -> usize {
        row.len()
    }

    fn row_string(&self, row: &Self::MetricRow, column: usize) -> String {
        match &row[column] {
            InspectionSummaryValue::String(value) => value.clone(),
            value => panic!("expected string at column {column}, got {value:?}"),
        }
    }

    fn row_float(&self, row: &Self::MetricRow, column: usize) -> f64 {
        match row[column] {
            InspectionSummaryValue::Float(value) => value,
            ref value => panic!("expected float at column {column}, got {value:?}"),
        }
    }
}

fn extractor(metric_name: &str, quantiles: Vec<f64>) -> InspectionSummaryExtractor {
    InspectionSummaryExtractor {
        skip_inspection: false,
        rules: HashSet::from(["query-summary".to_owned()]),
        metric_names: HashSet::from([metric_name.to_owned()]),
        quantiles,
    }
}

#[test]
/// 规则目录完整性：无空列表、无重复 metric、含 tidb_query_duration。
fn inspection_summary_rule_catalog_has_no_empty_or_duplicate_metrics() {
    let rules = inspectionSummaryRules();
    assert!(rules.contains_key("query-summary"));
    assert!(rules.contains_key("wait-events"));
    for metrics in rules.values() {
        assert!(!metrics.is_empty());
        let unique = metrics
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), metrics.len());
    }
    assert!(
        rules["query-summary"]
            .iter()
            .any(|metric| *metric == "tidb_query_duration")
    );
}

#[test]
fn inspection_summary_retrieve_matches_go_row_and_sql_semantics() {
    let mut runtime = MockRuntime::default();
    runtime.definitions.insert(
        "tidb_query_duration".to_owned(),
        MetricDefinition {
            labels: vec!["instance".to_owned(), "store".to_owned()],
            comment: "query duration".to_owned(),
            quantile: 1.0,
        },
    );
    runtime.rows.push(vec![
        InspectionSummaryValue::Float(2.0),
        InspectionSummaryValue::Float(1.0),
        InspectionSummaryValue::Float(3.0),
        InspectionSummaryValue::String("tikv-0".to_owned()),
        InspectionSummaryValue::String("42".to_owned()),
        InspectionSummaryValue::Float(0.9),
    ]);
    let mut retriever = InspectionSummaryRetriever {
        runtime,
        retrieved: false,
        extractor: extractor("tidb_query_duration", vec![0.9, 0.99]),
        time_range_condition: "where time >= 'start'".to_owned(),
    };

    let rows = retriever.retrieve(&mut ()).unwrap();

    assert_eq!(
        retriever.runtime.sql,
        vec![
            "select avg(value),min(value),max(value),`instance`,`store`,`quantile` from `metrics_schema`.`tidb_query_duration` where time >= 'start' and quantile in (0.900000,0.990000) group by `instance`,`store`,`quantile` order by `instance`,`store`,`quantile`"
        ]
    );
    assert_eq!(
        rows,
        vec![vec![
            InspectionSummaryValue::String("query-summary".to_owned()),
            InspectionSummaryValue::String("tikv-0".to_owned()),
            InspectionSummaryValue::String("tidb_query_duration".to_owned()),
            InspectionSummaryValue::String("store_id:42".to_owned()),
            InspectionSummaryValue::Float(0.9),
            InspectionSummaryValue::Float(2.0),
            InspectionSummaryValue::Float(1.0),
            InspectionSummaryValue::Float(3.0),
            InspectionSummaryValue::String("query duration".to_owned()),
        ]]
    );
    assert!(retriever.retrieve(&mut ()).unwrap().is_empty());
    assert_eq!(retriever.runtime.sql.len(), 1);
}

#[test]
fn inspection_summary_defaults_quantile_and_warns_for_missing_metrics() {
    let mut runtime = MockRuntime::default();
    runtime.definitions.insert(
        "tidb_query_duration".to_owned(),
        MetricDefinition {
            labels: Vec::new(),
            comment: String::new(),
            quantile: 1.0,
        },
    );
    let mut retriever = InspectionSummaryRetriever {
        runtime,
        retrieved: false,
        extractor: extractor("tidb_query_duration", Vec::new()),
        time_range_condition: "where time > now()".to_owned(),
    };
    retriever.retrieve(&mut ()).unwrap();
    assert!(retriever.runtime.sql[0].contains("quantile=0.99"));

    let mut missing = InspectionSummaryRetriever {
        runtime: MockRuntime::default(),
        retrieved: false,
        extractor: extractor("tidb_qps", Vec::new()),
        time_range_condition: String::new(),
    };
    assert!(missing.retrieve(&mut ()).unwrap().is_empty());
    assert_eq!(
        missing.runtime.warnings,
        vec!["metrics table: tidb_qps not found"]
    );
}

#[test]
fn inspection_summary_wraps_execution_error_with_sql_like_go() {
    let mut runtime = MockRuntime {
        error: Some("boom".to_owned()),
        ..MockRuntime::default()
    };
    runtime.definitions.insert(
        "tidb_qps".to_owned(),
        MetricDefinition {
            labels: Vec::new(),
            comment: String::new(),
            quantile: 0.0,
        },
    );
    let mut retriever = InspectionSummaryRetriever {
        runtime,
        retrieved: false,
        extractor: extractor("tidb_qps", Vec::new()),
        time_range_condition: "where time > now()".to_owned(),
    };

    let error = retriever.retrieve(&mut ()).unwrap_err();
    assert_eq!(
        error,
        "execute 'select avg(value),min(value),max(value) from `metrics_schema`.`tidb_qps` where time > now()' failed: boom"
    );
}
