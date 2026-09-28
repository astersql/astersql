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

// METRICS_SCHEMA 表定义一致性测试。
//
// 对应 `metrics_schema_test.go`。校验 MetricTableMap 中 PromQL 与 Quantile/Labels
// 的对应关系、表名小写、`instance` 标签位置，并用括号平衡近似替代 Go 的 PromQL 解析。
// PromQL：Prometheus 查询语言；histogram_quantile：直方图分位数聚合。

// 对应 pkg/infoschema/metrics_schema_test.go。
//
// Go 额外用 prometheus/promql/parser 校验替换后的 PromQL 语法。Rust 侧没有同款
// parser 依赖；这里保留全部结构性断言（quantile / labels / 表名大小写 /
// instance 首位），并用括号平衡检查近似替代 ParseExpr。

use std::collections::{HashMap, HashSet};

use crate::metric_table_def::MetricTableMap;
use crate::metrics_schema::{
    GenLabelConditionValues, GetMetricTableDef, IsMetricTable, MetricSchemaDBID, MetricTableDef,
    metric_schema_db,
};
use crate::tables::ColumnType;

/// 用固定占位值展开 PromQL 模板，供语法近似检查。
fn mock_gen_promql(prom_ql: &str) -> String {
    prom_ql
        .replace("$QUANTILE", "0.5")
        .replace("$LABEL_CONDITIONS", "")
        .replace("$RANGE_DURATION", "1s")
}

/// 检查圆括号/花括号/方括号是否平衡，近似 Go ParseExpr。
fn balanced_parens(expr: &str) -> bool {
    let mut paren = 0_i32;
    let mut brace = 0_i32;
    let mut bracket = 0_i32;
    for ch in expr.chars() {
        match ch {
            '(' => paren += 1,
            ')' => paren -= 1,
            '{' => brace += 1,
            '}' => brace -= 1,
            '[' => bracket += 1,
            ']' => bracket -= 1,
            _ => {}
        }
        if paren < 0 || brace < 0 || bracket < 0 {
            return false;
        }
    }
    paren == 0 && brace == 0 && bracket == 0
}

/// 遍历 MetricTableMap，校验分位数、标签、表名与 PromQL 结构约定。
#[test]
fn test_metric_schema_def() {
    for (name, def) in MetricTableMap.iter() {
        // 含 $QUANTILE 或 histogram_quantile 时 Quantile 必须 > 0。
        if def.PromQL.contains("$QUANTILE") || def.PromQL.contains("histogram_quantile") {
            assert!(
                def.Quantile > 0.0,
                "the quantile of metric table {name} should > 0"
            );
        } else {
            assert_eq!(
                0.0, def.Quantile,
                "metric table {name} has quantile, but doesn't contain $QUANTILE in promQL"
            );
        }

        if def.PromQL.contains("$LABEL_CONDITIONS") {
            assert!(
                !def.Labels.is_empty(),
                "the labels of metric table {name} should not be nil"
            );
        } else {
            let li = def.PromQL.find('{');
            let ri = def.PromQL.find('}');
            // ri - li > 1 means already has label conditions.
            // 花括号内已有固定标签时，允许 Labels 非空且无 $LABEL_CONDITIONS。
            let already_has_labels = matches!((li, ri), (Some(l), Some(r)) if r > l + 1);
            if !already_has_labels {
                assert!(
                    def.Labels.is_empty(),
                    "metric table {name} has labels, but doesn't contain $LABEL_CONDITIONS in promQL"
                );
            }
        }

        // `by (` 聚合分组时，Labels 中的每个标签名须出现在 PromQL 中。
        if def.PromQL.contains(" by (") {
            for label in def.Labels {
                assert!(
                    def.PromQL.contains(label),
                    "metric table {name} has labels, but doesn't contain label {label} in promQL"
                );
            }
        }

        assert_eq!(
            name.to_ascii_lowercase(),
            *name,
            "metric table name {name} should be lower case"
        );

        // INSTANCE must be the first label.
        // instance 若存在则必须是 Labels 的第一项，便于按实例过滤。
        if def.Labels.iter().any(|l| *l == "instance") {
            assert_eq!(
                "instance", def.Labels[0],
                "metrics table {name}: expect `instance` is the first label but got {:?}",
                def.Labels
            );
        }

        let expr = mock_gen_promql(def.PromQL);
        assert!(
            balanced_parens(&expr),
            "fail to parse PromQL (unbalanced delimiters) for {name}: {expr}"
        );
        assert!(!expr.is_empty(), "PromQL for {name} should not be empty");
    }
}

/// Rust 进程不应继续暴露 Go GC/goroutine 表；内存与线程改用
/// rust-prometheus 在 Linux 上默认注册的进程指标。
#[test]
fn runtime_metric_catalog_uses_rust_process_metrics() {
    for go_only_table in [
        "go_gc_count",
        "go_gc_cpu_usage",
        "go_gc_duration",
        "go_heap_mem_usage",
        "go_threads",
        "goroutines_count",
    ] {
        assert!(
            !MetricTableMap.contains_key(go_only_table),
            "Go-only runtime metric table must be removed: {go_only_table}"
        );
    }

    let memory = MetricTableMap
        .get("rust_process_mem_usage")
        .expect("Rust process memory table must be registered");
    assert_eq!(
        memory.PromQL,
        "process_resident_memory_bytes{$LABEL_CONDITIONS}"
    );
    assert_eq!(memory.Labels, &["instance", "job"]);

    let threads = MetricTableMap
        .get("rust_process_threads")
        .expect("Rust process thread table must be registered");
    assert_eq!(threads.PromQL, "process_threads{$LABEL_CONDITIONS}");
    assert_eq!(threads.Labels, &["instance", "job"]);
}

#[test]
fn metric_table_helpers_match_go_contract() {
    let definition = MetricTableDef {
        PromQL: "histogram_quantile($QUANTILE, sum(rate(metric{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance))",
        Labels: &["instance", "type"],
        Quantile: 0.99,
        Comment: "latency",
    };

    let columns = definition.genColumnInfos();
    assert_eq!(
        columns.iter().map(|column| column.name).collect::<Vec<_>>(),
        ["time", "instance", "type", "quantile", "value"]
    );
    assert_eq!(columns[0].column_type, ColumnType::Datetime);
    assert_eq!(columns[0].size, 19);
    assert_eq!(columns[0].default_value, Some("CURRENT_TIMESTAMP"));
    assert_eq!(columns[1].column_type, ColumnType::Varchar);
    assert_eq!(columns[1].size, 512);
    assert_eq!(columns[3].column_type, ColumnType::Double);
    assert_eq!(columns[3].size, 22);
    assert_eq!(columns[3].default_value, Some("0.99"));

    let labels = HashMap::from([
        (
            "type".to_owned(),
            HashSet::from(["write".to_owned(), "read".to_owned()]),
        ),
        ("instance".to_owned(), HashSet::from(["tidb-0".to_owned()])),
        ("ignored".to_owned(), HashSet::from(["value".to_owned()])),
    ]);
    assert_eq!(
        definition.GenPromQL(60, &labels, 0.95),
        "histogram_quantile(0.95, sum(rate(metric{instance=\"tidb-0\",type=~\"read|write\"}[60s])) by (instance))"
    );
    assert_eq!(
        GenLabelConditionValues(&HashSet::from([
            "z".to_owned(),
            "a".to_owned(),
            "m".to_owned()
        ])),
        "a|m|z"
    );

    assert!(IsMetricTable("rust_process_threads"));
    assert!(!IsMetricTable("RUST_PROCESS_THREADS"));
    assert_eq!(
        GetMetricTableDef("missing").unwrap_err(),
        "can not find metric table: missing"
    );
}

#[test]
fn metric_schema_metadata_has_stable_sorted_ids() {
    let database = metric_schema_db();
    assert_eq!(database.id, MetricSchemaDBID);
    assert_eq!(database.name.original, "METRICS_SCHEMA");
    assert!(database.tables.windows(2).all(|tables| {
        tables[0].name.lower < tables[1].name.lower && tables[1].id == tables[0].id + 1
    }));
    assert_eq!(
        database.tables.first().map(|table| table.id),
        Some(MetricSchemaDBID + 1)
    );
    assert!(
        database
            .tables
            .iter()
            .all(|table| table.db_id == MetricSchemaDBID)
    );
}
