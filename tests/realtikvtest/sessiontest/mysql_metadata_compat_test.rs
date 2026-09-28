// Copyright 2026 AsterSQL.

// MySQL 元数据兼容性清单的 realtikv 会话层回归测试。
//
// 本文件读取跨测试层共享的兼容性用例，仅执行要求由 canonical session
// 承担的部分，并在 realtikv sessiontest 的 TestKit 通路中核对结果或错误边界。

use serde_json::Value;

use astersql_testkit::NewTestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_tests_realtikvtest_sessiontest::serial_guard;

const MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../mysqlcompat/compatibility-cases.json"
));

// 严格解析清单中的字符串数组，使格式错误直接指向具体用例和字段。
fn string_array(value: &Value, field: &str, case_id: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{case_id}.{field} must be an array"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("{case_id}.{field} entries must be strings"))
                .to_owned()
        })
        .collect()
}

// 将二维字符串数组转换为 TestKit 使用的行表示，并保留清单声明的列顺序。
fn expected_rows(value: &Value, field: &str, case_id: &str) -> Vec<Vec<String>> {
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

// 只应用清单约定的确定性归一化，避免大小写和无序结果造成伪回归。
fn normalize(mut rows: Vec<Vec<String>>, normalizers: &[String]) -> Vec<Vec<String>> {
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
fn mysql_metadata_manifest_runs_through_realtikv_sessiontest() {
    // 串行保护共享的 RealTiKV 测试夹具及进程级会话配置，避免用例相互污染。
    let _serial = serial_guard();
    let cases = serde_json::from_str::<Vec<Value>>(MANIFEST).expect("parse compatibility manifest");
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = NewTestKit(store);
    let mut executed = 0_usize;

    for case in cases {
        // 其他层或非强制用例由各自测试入口负责，避免在此重复覆盖。
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
        let normalizers = string_array(&case, "normalizers", case_id);

        for setup_sql in string_array(&case, "setup_sql", case_id) {
            testkit.MustExec(&setup_sql, Vec::new());
        }

        if let Some(error) = expected.get("error").and_then(Value::as_object) {
            // 错误清单只约束稳定片段，不绑定可能随实现变化的完整错误文本。
            let actual = testkit.QueryToErr(query).message().to_owned();
            for fragment in string_array(&Value::Object(error.clone()), "message_contains", case_id)
            {
                assert!(
                    actual.contains(&fragment),
                    "{case_id}.error missing {fragment:?}; actual={actual:?}"
                );
            }
            executed += 1;
            continue;
        }

        let query_rows = testkit
            .Query(query, Vec::new())
            .unwrap_or_else(|error| panic!("{case_id}.query failed: {query}: {error}"));
        if expected.contains_key("columns") {
            let wanted = string_array(&Value::Object(expected.clone()), "columns", case_id);
            let actual = if normalizers.iter().any(|item| item == "lowercase_names") {
                query_rows
                    .columns
                    .iter()
                    .map(|column| column.to_ascii_lowercase())
                    .collect::<Vec<_>>()
            } else {
                query_rows.columns.clone()
            };
            assert_eq!(actual, wanted, "{case_id}.expected.columns mismatch");
        }
        let actual = normalize(query_rows.string_rows(), &normalizers);
        // rows 要求完整相等；contains_rows 只要求关键元数据行存在。
        if expected.contains_key("rows") {
            let wanted = normalize(
                expected_rows(&Value::Object(expected.clone()), "rows", case_id),
                &normalizers,
            );
            assert_eq!(actual, wanted, "{case_id}.expected.rows mismatch");
        }
        if expected.contains_key("contains_rows") {
            let wanted = normalize(
                expected_rows(&Value::Object(expected.clone()), "contains_rows", case_id),
                &normalizers,
            );
            let missing = wanted
                .iter()
                .filter(|row| !actual.contains(row))
                .cloned()
                .collect::<Vec<_>>();
            assert!(
                missing.is_empty(),
                "{case_id}.expected.contains_rows missing={missing:?}; actual={actual:?}"
            );
        }
        executed += 1;
    }

    // 固定数量是清单覆盖哨兵，防止必测用例被改标签或静默移除。
    assert_eq!(
        executed, 11,
        "required canonical-session manifest case count changed"
    );
    println!("realtikv session compatibility manifest: {executed} cases passed");
}
