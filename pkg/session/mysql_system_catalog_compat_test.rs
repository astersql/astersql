// Copyright 2026 AsterSQL.

//! MySQL 系统目录兼容性清单的规范会话回归测试。
//!
//! 本模块读取跨实现共享的兼容性用例，只执行标记为必需且属于
//! `canonical_session` 层的场景；覆盖查询成功、预期报错、列名以及完整或子集行断言。
//! 最后再枚举四个系统 schema，防止清单用例通过但目录对象整体缺失。

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::runtime::{ConcreteRecordSet, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

/// 编译期嵌入共享清单，避免测试结果依赖运行时工作目录。
const MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/mysqlcompat/compatibility-cases.json"
));

/// 耗尽结果集并保留清单断言使用的字符串行表示。
fn collect(mut result: ConcreteRecordSet) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    while let Some(row) = result.Next().expect("read manifest result row") {
        rows.push(row);
    }
    rows
}

/// 从清单字段读取字符串数组；错误信息带用例编号，便于定位格式问题。
fn strings(value: &Value, field: &str, case_id: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{case_id}.{field} must be an array"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("{case_id}.{field} entries must be strings"))
                .to_owned()
        })
        .collect()
}

/// 从清单字段读取二维字符串数组，并严格拒绝非字符串单元格。
fn rows(value: &Value, field: &str, case_id: &str) -> Vec<Vec<String>> {
    value
        .get(field)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{case_id}.{field} must be an array"))
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap_or_else(|| panic!("{case_id}.{field} entries must be row arrays"))
                .iter()
                .map(|cell| {
                    cell.as_str()
                        .unwrap_or_else(|| panic!("{case_id}.{field} cells must be strings"))
                        .to_owned()
                })
                .collect()
        })
        .collect()
}

/// 按清单声明统一实际值与期望值，消除名称大小写和行顺序的非语义差异。
fn normalize_rows(mut rows: Vec<Vec<String>>, normalizers: &[String]) -> Vec<Vec<String>> {
    if normalizers.iter().any(|item| item == "lowercase_names") {
        for row in &mut rows {
            for cell in row {
                *cell = cell.to_ascii_lowercase();
            }
        }
    }
    if normalizers.iter().any(|item| item == "sort_rows") {
        rows.sort();
    }
    rows
}

#[test]
fn mysql_system_catalog_manifest_matches_canonical_session() {
    let cases = serde_json::from_str::<Vec<Value>>(MANIFEST).expect("parse compatibility manifest");
    let (_domain, session) = CreateAnalyzeSession().expect("canonical catalog session");
    let mut executed = 0_usize;

    for case in cases {
        // 其他层或非必需场景由各自测试负责，避免扩大本测试的兼容性承诺。
        if case.get("layer").and_then(Value::as_str) != Some("canonical_session")
            || case.get("required").and_then(Value::as_bool) != Some(true)
        {
            continue;
        }

        let case_id = case["id"].as_str().expect("manifest case id");
        let query = case["query"].as_str().expect("manifest query");
        let expected = case["expected"]
            .as_object()
            .unwrap_or_else(|| panic!("{case_id}.expected must be an object"));
        let normalizers = strings(&case, "normalizers", case_id);

        // 前置 SQL 与查询共用同一规范会话，以保留用例要求的会话状态。
        for setup_sql in strings(&case, "setup_sql", case_id) {
            session
                .execute(&setup_sql)
                .unwrap_or_else(|error| panic!("{case_id}.setup_sql failed: {setup_sql}: {error}"));
        }

        // 报错用例只校验稳定的消息片段，避免绑定完整错误包装文本。
        if let Some(error) = expected.get("error").and_then(Value::as_object) {
            let actual = match session.execute(query) {
                Ok(_) => panic!("{case_id}.error: query unexpectedly succeeded: {query}"),
                Err(error) => error.to_string(),
            };
            for fragment in strings(&Value::Object(error.clone()), "message_contains", case_id) {
                assert!(
                    actual.contains(&fragment),
                    "{case_id}.error.message_contains missing {fragment:?}; actual={actual:?}",
                );
            }
            executed += 1;
            continue;
        }

        // 成功用例可分别约束列名、完整结果或必须出现的结果子集。
        let mut result = session
            .execute(query)
            .unwrap_or_else(|error| panic!("{case_id}.query failed: {query}: {error}"))
            .remove(0);
        if expected.contains_key("columns") {
            let expected_columns = strings(&Value::Object(expected.clone()), "columns", case_id);
            let actual_columns = result.columns();
            assert_eq!(
                actual_columns
                    .iter()
                    .map(|column| column.to_ascii_lowercase())
                    .collect::<Vec<_>>(),
                expected_columns
                    .iter()
                    .map(|column| column.to_ascii_lowercase())
                    .collect::<Vec<_>>(),
                "{case_id}.expected.columns mismatch; actual={actual_columns:?}",
            );
        }

        let actual = normalize_rows(collect(result), &normalizers);
        if expected.contains_key("rows") {
            let wanted = normalize_rows(
                rows(&Value::Object(expected.clone()), "rows", case_id),
                &normalizers,
            );
            assert_eq!(actual, wanted, "{case_id}.expected.rows mismatch");
        }
        if expected.contains_key("contains_rows") {
            let wanted = normalize_rows(
                rows(&Value::Object(expected.clone()), "contains_rows", case_id),
                &normalizers,
            );
            let missing = wanted
                .iter()
                .filter(|row| !actual.contains(row))
                .cloned()
                .collect::<Vec<_>>();
            assert!(
                missing.is_empty(),
                "{case_id}.expected.contains_rows missing={missing:?}; actual={actual:?}",
            );
        }
        executed += 1;
    }

    // 固定数量用于捕获必需用例被误删、改层级或降级为可选的清单漂移。
    assert_eq!(
        executed, 11,
        "required canonical-session manifest case count changed"
    );
    // 清单断言之外再做目录可发现性兜底，确保每个系统 schema 至少暴露一个对象。
    let mut object_counts = BTreeMap::new();
    for schema in ["mysql", "information_schema", "performance_schema", "sys"] {
        let result = session
            .execute(&format!("show full tables from {schema}"))
            .unwrap_or_else(|error| panic!("enumerate {schema} objects: {error}"))
            .remove(0);
        let count = collect(result).len();
        assert!(
            count > 0,
            "{schema} must expose at least one required object"
        );
        object_counts.insert(schema, count);
    }
    println!(
        "canonical system catalog manifest: {executed} required cases passed; objects={object_counts:?}"
    );
}
