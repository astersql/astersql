// Copyright 2026 AsterSQL.

// MySQL 目录查询的线协议兼容性测试。
//
// 测试从共享兼容性清单中选取必须通过真实 TCP 连接验证的用例，检查查询结果、
// ColumnDefinition41 元数据与状态标志，并分别覆盖传统 EOF 包和弃用 EOF 包两种模式。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::mysql_compat_test_support::{
    CLIENT_DEPRECATE_EOF, ColumnDefinition, MysqlCompatServer, MysqlTestClient, TextResultSet,
    TextValue, WireResponse,
};

const MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/mysqlcompat/compatibility-cases.json"
));
const MYSQL_TYPE_LONG: u8 = 0x03;
const MYSQL_TYPE_LONGLONG: u8 = 0x08;
const MYSQL_TYPE_VAR_STRING: u8 = 0xfd;
const NOT_NULL_FLAG: u16 = 0x0001;
const PRI_KEY_FLAG: u16 = 0x0002;
const UNSIGNED_FLAG: u16 = 0x0020;
const AUTO_INCREMENT_FLAG: u16 = 0x0200;
const SERVER_STATUS_AUTOCOMMIT: u16 = 0x0002;

fn tcp_cases() -> Vec<Value> {
    // 同一份清单也服务于其他协议层；这里只执行要求经过 TCP 线协议验证的必选用例。
    serde_json::from_str::<Vec<Value>>(MANIFEST)
        .expect("parse MySQL compatibility manifest")
        .into_iter()
        .filter(|case| {
            case.get("layer").and_then(Value::as_str) == Some("tcp")
                && case.get("required").and_then(Value::as_bool) == Some(true)
        })
        .collect()
}

fn strings(value: &Value, field: &str, case_id: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{case_id}.{field} must be an array"))
        .iter()
        .map(|item| {
            item.as_str()
                .unwrap_or_else(|| panic!("{case_id}.{field} entries must be strings"))
                .to_owned()
        })
        .collect()
}

fn rows(value: &Value, field: &str, case_id: &str) -> Vec<Vec<String>> {
    value
        .get(field)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{case_id}.{field} must be an array"))
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap_or_else(|| panic!("{case_id}.{field} entries must be rows"))
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

fn expect_ok(response: WireResponse, case_id: &str, sql: &str) {
    assert!(
        matches!(response, WireResponse::Ok(_)),
        "{case_id}.setup_sql failed: {sql}: {response:?}",
    );
}

fn query(client: &mut MysqlTestClient, case_id: &str, sql: &str) -> TextResultSet {
    match client
        .query(sql)
        .unwrap_or_else(|error| panic!("{case_id}.query transport failure: {sql}: {error}"))
    {
        WireResponse::ResultSet(result) => result,
        response => panic!("{case_id}.query expected result set: {sql}: {response:?}"),
    }
}

fn text_rows(result: &TextResultSet) -> Vec<Vec<String>> {
    result
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| match value {
                    TextValue::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
                    TextValue::Null => "<NULL>".to_owned(),
                })
                .collect()
        })
        .collect()
}

fn column<'a>(columns: &'a [ColumnDefinition], name: &str, case_id: &str) -> &'a ColumnDefinition {
    columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("{case_id}.expected.{name} missing; actual={columns:?}"))
}

fn assert_flag(column: &ColumnDefinition, flag: &str, case_id: &str) {
    let mask = match flag {
        "NOT_NULL" => NOT_NULL_FLAG,
        "PRI_KEY" => PRI_KEY_FLAG,
        "UNSIGNED" => UNSIGNED_FLAG,
        "AUTO_INCREMENT" => AUTO_INCREMENT_FLAG,
        other => panic!("{case_id}: unknown manifest column flag {other}"),
    };
    assert_eq!(
        column.flags & mask,
        mask,
        "{case_id}.expected.{}.flags missing {flag}; actual={column:?}",
        column.name,
    );
}

fn assert_column_contract(actual: &ColumnDefinition, expected: &Value, case_id: &str, name: &str) {
    // 清单只声明当前用例关心的字段，未声明的元数据不应被强行约束。
    for (field, actual_value) in [
        ("catalog", actual.catalog.as_str()),
        ("schema", actual.schema.as_str()),
        ("table", actual.table.as_str()),
        ("org_table", actual.org_table.as_str()),
        ("org_name", actual.org_name.as_str()),
    ] {
        if let Some(wanted) = expected.get(field).and_then(Value::as_str) {
            assert_eq!(
                actual_value, wanted,
                "{case_id}.expected.{name}.{field} mismatch; column={actual:?}",
            );
        }
    }
    if let Some(wanted) = expected.get("column_length").and_then(Value::as_u64) {
        assert_eq!(
            u64::from(actual.column_length),
            wanted,
            "{case_id}.expected.{name}.column_length mismatch; column={actual:?}",
        );
    }
    if let Some(wanted) = expected.get("decimals").and_then(Value::as_u64) {
        assert_eq!(
            u64::from(actual.decimals),
            wanted,
            "{case_id}.expected.{name}.decimals mismatch; column={actual:?}",
        );
    }
    if let Some(wanted) = expected.get("type").and_then(Value::as_str) {
        let wanted = match wanted {
            "MYSQL_TYPE_LONG" => MYSQL_TYPE_LONG,
            "MYSQL_TYPE_LONGLONG" => MYSQL_TYPE_LONGLONG,
            "MYSQL_TYPE_VAR_STRING" => MYSQL_TYPE_VAR_STRING,
            other => panic!("{case_id}.expected.{name}.type unknown: {other}"),
        };
        assert_eq!(
            actual.column_type, wanted,
            "{case_id}.expected.{name}.type mismatch; column={actual:?}",
        );
    }
    if expected.get("flags").is_some() {
        for flag in strings(expected, "flags", case_id) {
            assert_flag(actual, &flag, case_id);
        }
    }
}

fn run_manifest(capabilities: u32) -> usize {
    let mut executed = 0;

    for case in tcp_cases() {
        let case_id = case["id"].as_str().expect("TCP manifest case id");
        let server = MysqlCompatServer::start()
            .unwrap_or_else(|error| panic!("{case_id}: start real listener: {error}"));
        let mut client = server
            .connect_root(capabilities, None)
            .unwrap_or_else(|error| panic!("{case_id}: connect real listener: {error}"));
        // Go's mock store bootstraps root with global privileges.  The Rust
        // real-listener fixture deliberately only bypasses root authentication,
        // so materialize the equivalent privilege rows before exercising
        // catalog tables through the production privilege checker.
        for sql in [
            "CREATE USER IF NOT EXISTS 'root'@'%'",
            "GRANT ALL PRIVILEGES ON *.* TO 'root'@'%'",
        ] {
            let response = client.query(sql).unwrap_or_else(|error| {
                panic!("{case_id}.root bootstrap transport failure: {error}")
            });
            expect_ok(response, case_id, sql);
        }
        for sql in strings(&case, "setup_sql", case_id) {
            let response = client
                .query(&sql)
                .unwrap_or_else(|error| panic!("{case_id}.setup_sql transport failure: {error}"));
            expect_ok(response, case_id, &sql);
        }

        let sql = case["query"].as_str().expect("TCP manifest query");
        let expected = &case["expected"];
        let result = query(&mut client, case_id, sql);
        assert_ne!(
            result.status & SERVER_STATUS_AUTOCOMMIT,
            0,
            "{case_id}.status must preserve SERVER_STATUS_AUTOCOMMIT",
        );

        if expected.get("rows").is_some() {
            let mut actual = text_rows(&result);
            let mut wanted = rows(expected, "rows", case_id);
            // 仅在清单明确允许时消除行序差异，避免掩盖本应稳定的返回顺序。
            if strings(&case, "normalizers", case_id)
                .iter()
                .any(|item| item == "sort_rows")
            {
                actual.sort();
                wanted.sort();
            }
            assert_eq!(actual, wanted, "{case_id}.expected.rows mismatch");
        }
        if expected.get("contains_objects").is_some() {
            // 目录查询可能返回额外系统对象，因此这里只验证必需对象及其类型。
            let objects = text_rows(&result)
                .into_iter()
                .map(|row| (row[0].to_ascii_lowercase(), row[1].to_ascii_uppercase()))
                .collect::<BTreeMap<_, _>>();
            let object_type = expected["object_type"].as_str().expect("object type");
            for object in strings(expected, "contains_objects", case_id) {
                assert_eq!(
                    objects.get(&object).map(String::as_str),
                    Some(object_type),
                    "{case_id}.expected.contains_objects missing {object}; actual={objects:?}",
                );
            }
        }
        for (name, contract) in expected
            .as_object()
            .expect("TCP expected object")
            .iter()
            .filter(|(_, value)| value.is_object())
        {
            assert_column_contract(
                column(&result.columns, name, case_id),
                contract,
                case_id,
                name,
            );
        }
        if expected.get("row_values").and_then(Value::as_str) == Some("ascii_decimal_integers") {
            // 文本协议中的整数必须编码为十进制 ASCII，而不是二进制整数载荷。
            for name in ["Id", "Time"] {
                let offset = result
                    .columns
                    .iter()
                    .position(|column| column.name == name)
                    .unwrap_or_else(|| panic!("{case_id}: missing {name}"));
                assert!(
                    result.rows.iter().all(|row| matches!(
                        &row[offset], TextValue::Bytes(value) if value.iter().all(u8::is_ascii_digit)
                    )),
                    "{case_id}.expected.row_values invalid {name}: {:?}",
                    result.rows,
                );
            }
        }

        if case_id == "protocol.column-definition41-identity" {
            // 对同一查询路径交叉检查 NULL 编码、派生表投影和预处理语句元数据，
            // 防止执行方式变化时丢失原始列身份或改变线协议表示。
            expect_ok(
                client
                    .query("insert into records(note) values (NULL)")
                    .expect("insert NULL metadata row"),
                case_id,
                "insert NULL metadata row",
            );
            let direct = query(&mut client, case_id, "select id, note from records");
            assert_eq!(
                direct.rows,
                vec![vec![TextValue::Bytes(b"1".to_vec()), TextValue::Null]],
                "{case_id}.text NULL/value encoding mismatch",
            );
            let derived = query(
                &mut client,
                case_id,
                "select id, note from (select id, note from records) as derived_records",
            );
            for name in ["id", "note"] {
                let source = column(&direct.columns, name, case_id);
                let projected = column(&derived.columns, name, case_id);
                assert_eq!(projected.column_type, source.column_type);
                assert_eq!(projected.org_name, source.org_name);
                assert_eq!(projected.schema, source.schema);
            }
            assert_eq!(derived.rows, direct.rows, "{case_id}.derived rows mismatch");

            let prepared = client
                .prepare("select id, note from records")
                .expect("prepare identity SELECT over real TCP");
            assert_eq!(
                prepared.columns, direct.columns,
                "{case_id}.prepared metadata mismatch"
            );
        }
        executed += 1;
    }
    executed
}

#[test]
fn mysql_catalog_protocol_manifest_matches_wire_metadata() {
    // 固定必选用例集合，避免清单筛选条件意外导致协议覆盖面静默缩小。
    let ids = tcp_cases()
        .iter()
        .map(|case| case["id"].as_str().expect("TCP case ID").to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 6, "required TCP manifest case count changed");

    // 每个用例在两种 EOF 协商模式下都必须产生相同的目录与列元数据契约。
    let classic = run_manifest(0);
    let deprecate_eof = run_manifest(CLIENT_DEPRECATE_EOF);
    assert_eq!(classic, 6);
    assert_eq!(deprecate_eof, 6);
    println!(
        "mysql catalog protocol manifest: {} cases x 2 EOF modes = {} wire scenarios",
        ids.len(),
        classic + deprecate_eof,
    );
}
