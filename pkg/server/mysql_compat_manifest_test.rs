// Copyright 2026 AsterSQL.

// MySQL 兼容性用例清单的静态契约测试。
//
// 本文件不执行清单中的 SQL，而是在编译期嵌入共享 JSON 清单，并校验其结构、
// 证据来源、支持状态及首批覆盖范围，避免各执行层消费到含义不一致的用例。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

const MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/mysqlcompat/compatibility-cases.json"
));

// 清单字段和值域必须保持封闭；新增字段或枚举值时需同步更新消费端契约。
const REQUIRED_FIELDS: [&str; 9] = [
    "id",
    "source",
    "required",
    "layer",
    "setup_sql",
    "query",
    "expected",
    "normalizers",
    "evidence",
];
const SOURCES: [&str; 3] = ["go", "mysql80", "shared"];
const LAYERS: [&str; 4] = [
    "canonical_session",
    "tcp",
    "integration",
    "realtikv_session",
];
const NORMALIZERS: [&str; 5] = [
    "sort_rows",
    "lowercase_names",
    "normalize_dynamic_id",
    "normalize_timestamp",
    "normalize_address",
];

/// 将嵌入的清单解析为逐项可校验的 JSON 对象。
fn cases() -> Vec<Map<String, Value>> {
    serde_json::from_str::<Vec<Map<String, Value>>>(MANIFEST)
        .expect("tests/mysqlcompat/compatibility-cases.json must be a JSON array of objects")
}

/// 读取必填字符串字段，并让类型错误直接指向完整用例。
fn string_field<'a>(case: &'a Map<String, Value>, field: &str) -> &'a str {
    case.get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("manifest field {field} must be a string: {case:?}"))
}

/// 读取字符串数组字段；数组本身或任一元素类型错误都会立即失败。
fn string_array<'a>(case: &'a Map<String, Value>, field: &str) -> Vec<&'a str> {
    case.get(field)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("manifest field {field} must be an array: {case:?}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("manifest {field} entries must be strings: {case:?}"))
        })
        .collect()
}

/// 校验基础模式、稳定且唯一的用例 ID，以及各封闭值域。
#[test]
fn manifest_is_well_formed_and_ids_are_unique() {
    let cases = cases();
    assert!(
        !cases.is_empty(),
        "compatibility manifest must not be empty"
    );

    let mut ids = BTreeSet::new();
    for case in &cases {
        assert_eq!(
            case.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            REQUIRED_FIELDS.into_iter().collect(),
            "manifest cases must use the fixed field set: {case:?}",
        );

        let id = string_field(case, "id");
        assert!(
            !id.is_empty()
                && id.bytes().all(|byte| byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || b".-".contains(&byte)),
            "manifest ID must be stable lowercase dotted text: {id}",
        );
        assert!(ids.insert(id), "duplicate compatibility manifest ID: {id}");

        let source = string_field(case, "source");
        assert!(
            SOURCES.contains(&source),
            "invalid source {source} for {id}"
        );
        let layer = string_field(case, "layer");
        assert!(LAYERS.contains(&layer), "invalid layer {layer} for {id}");
        assert!(
            !string_field(case, "query").trim().is_empty(),
            "empty query for {id}"
        );

        for setup in string_array(case, "setup_sql") {
            assert!(!setup.trim().is_empty(), "empty setup_sql entry for {id}");
        }
        for normalizer in string_array(case, "normalizers") {
            assert!(
                NORMALIZERS.contains(&normalizer),
                "unknown normalizer {normalizer} for {id}",
            );
        }
    }
}

/// 确保每个结论都有证据，且 required 与 unsupported 状态严格对应。
#[test]
fn required_cases_have_evidence_and_determinate_expectations() {
    for case in cases() {
        let id = string_field(&case, "id");
        let required = case
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| panic!("required must be boolean for {id}"));
        let expected = case
            .get("expected")
            .and_then(Value::as_object)
            .filter(|expected| !expected.is_empty())
            .unwrap_or_else(|| panic!("expected must be a non-empty object for {id}"));
        let evidence = string_array(&case, "evidence");

        assert!(
            !evidence.is_empty() && evidence.iter().all(|item| !item.trim().is_empty()),
            "every case needs concrete source evidence: {id}",
        );
        if required {
            assert_ne!(
                expected.get("status").and_then(Value::as_str),
                Some("unsupported"),
                "required case cannot be unsupported: {id}",
            );
        } else {
            assert_eq!(
                expected.get("status").and_then(Value::as_str),
                Some("unsupported"),
                "non-required case must explicitly declare unsupported: {id}",
            );
            assert!(
                expected
                    .get("reason")
                    .and_then(Value::as_str)
                    .is_some_and(|reason| !reason.trim().is_empty()),
                "unsupported case needs a reason: {id}",
            );
        }

        if string_field(&case, "source") == "mysql80" {
            assert!(
                evidence
                    .iter()
                    .any(|item| item.starts_with("https://dev.mysql.com/")),
                "mysql80 case needs official MySQL evidence: {id}",
            );
        }
        if required && string_field(&case, "source") == "shared" {
            assert!(
                evidence
                    .iter()
                    .any(|item| item.starts_with("https://dev.mysql.com/")),
                "required shared case needs official protocol evidence: {id}",
            );
            assert!(
                evidence.iter().any(|item| !item.contains("://")),
                "required shared case needs a local regression-test anchor: {id}",
            );
        }
    }
}

/// 防止首批清单在来源、执行层或兼容类别上出现覆盖缺口。
#[test]
fn first_batch_covers_each_source_layer_and_contract_category() {
    let cases = cases();
    let mut sources = BTreeMap::<&str, usize>::new();
    let mut layers = BTreeMap::<&str, usize>::new();
    let mut categories = BTreeSet::new();

    for case in &cases {
        *sources.entry(string_field(case, "source")).or_default() += 1;
        *layers.entry(string_field(case, "layer")).or_default() += 1;
        categories.insert(
            string_field(case, "id")
                .split_once('.')
                .map_or(string_field(case, "id"), |(category, _)| category),
        );
    }

    assert_eq!(
        sources.keys().copied().collect::<BTreeSet<_>>(),
        SOURCES.into_iter().collect()
    );
    assert!(layers.contains_key("canonical_session"));
    assert!(layers.contains_key("tcp"));
    for category in [
        "catalog",
        "information-schema",
        "mysql",
        "performance-schema",
        "sys",
        "protocol",
    ] {
        assert!(
            categories.contains(category),
            "missing first-batch category {category}"
        );
    }

    println!(
        "mysql compatibility manifest: {} cases; sources={sources:?}; layers={layers:?}; categories={categories:?}",
        cases.len(),
    );
}
