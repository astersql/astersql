// Copyright 2026 AsterSQL.

//! Parity tests for `tests/llmtest/testcase` public contracts vs Go.

// 本文件对应 `tests/llmtest/testcase/parity_test.rs`，本次任务只补中文解释，不改行为。
// 本文件按成功、边界、错误和清理四类场景组织。
// 阅读时先看总入口，再看各个合同分组。
// 这里的目标是证明 Rust 与 Go 的公共合同一致。
// 成功路径关注正常返回值和可观测副作用。
// 边界路径关注空输入、默认值和最小变体。
// 错误路径关注日志、panic、exit 与错误文本。
// 清理路径关注 Close、Sync、Join 和资源释放。
// 中文注释优先解释为什么要断言。
// 补充阅读提示 1：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 2：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 3：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 4：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 5：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 6：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
// 补充阅读提示 7：错误路径要同时看返回值、日志和是否提前终止。
// 补充阅读提示 8：边界路径通常说明默认值、空输入和最小可用配置。
// 补充阅读提示 9：成功路径通常说明副作用被记录在什么位置。
// 补充阅读提示 10：如果出现桩对象，优先看它暴露了哪些可观测状态。
// 补充阅读提示 11：这些中文不会改变断言，只帮助缩短重新入场时间。
// 补充阅读提示 12：当多个 helper 串联时，顺序本身往往就是语义的一部分。
// 补充阅读提示 13：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 14：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 15：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 16：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 17：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 18：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
// 补充阅读提示 19：错误路径要同时看返回值、日志和是否提前终止。
// 补充阅读提示 20：边界路径通常说明默认值、空输入和最小可用配置。
// 补充阅读提示 21：成功路径通常说明副作用被记录在什么位置。
// 补充阅读提示 22：如果出现桩对象，优先看它暴露了哪些可观测状态。
use crate::stubs::{AnyValue, Db, QueryOutcome, SqlError, json};
use crate::{Case, Manager, open};
use astersql_tests_llmtest_logger::{Global, ensure_init};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

// 测试 `go_rust_public_contract_matches` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
#[test]
// `go_rust_public_contract_matches` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn go_rust_public_contract_matches() {
    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

// `tmp_path` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn tmp_path(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("llmtest-testcase-{label}-{nanos}.json"))
}

/// Normal: Open/Append/Save/Exist/AllGroups + matching A/B results mark pass.
// `contract_normal_paths` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_normal_paths() {
    ensure_init();
    let path = tmp_path("normal");
    fs::write(
        &path,
        r#"{
  "g1": [
    {
      "sql": "SELECT 1",
      "args": null,
      "pass": false,
      "known": false,
      "comment": ""
    }
  ]
}
"#,
    )
    .unwrap();

    let m = open(path.to_string_lossy()).unwrap();
    assert_eq!(m.exist_cases("g1").len(), 1);
    assert!(m.all_groups().contains(&"g1".to_string()));

    m.append_case(
        "g2",
        Case {
            sql: "SELECT ?;SELECT 2".to_string(),
            args: Some(vec![AnyValue::Number("7".into())]),
            pass: false,
            known: false,
            comment: "multi".into(),
        },
    );
    assert_eq!(m.exist_cases("g2").len(), 1);
    m.save().unwrap();

    let reloaded = open(path.to_string_lossy()).unwrap();
    let g2 = reloaded.exist_cases("g2");
    assert_eq!(g2.len(), 1);
    assert_eq!(g2[0].sql, "SELECT ?;SELECT 2");
    assert_eq!(g2[0].args.as_ref().unwrap().len(), 1);
    assert_eq!(g2[0].comment, "multi");

    let db = Db::always_ok(QueryOutcome {
        columns: vec!["c".into()],
        rows: vec![vec![Some("1".into())]],
        rows_err: None,
    });
    let m2 = open(path.to_string_lossy()).unwrap();
    m2.run_ab_test(&db, &db, true);
    let c = &m2.exist_cases("g1")[0];
    assert!(c.pass, "identical DB results must pass");

    let _ = fs::remove_file(&path);
}

/// Boundary: empty SQL fragments skipped; known/pass skip rules; HTML escape.
// `contract_boundary` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_boundary() {
    ensure_init();
    let path = tmp_path("boundary");
    let m = Manager::new_empty(path.to_string_lossy());

    m.append_case(
        "b",
        Case {
            sql: "SELECT NULL;".to_string(),
            args: None,
            pass: false,
            known: false,
            comment: String::new(),
        },
    );

    let db = Db::always_ok(QueryOutcome {
        columns: vec!["v".into()],
        rows: vec![vec![None]],
        rows_err: None,
    });
    m.run_ab_test(&db, &db, true);
    assert!(m.exist_cases("b")[0].pass);
    assert_eq!(db.query_log().len(), 2);

    m.append_case(
        "b",
        Case {
            sql: "SELECT 9".into(),
            args: None,
            pass: false,
            known: true,
            comment: String::new(),
        },
    );
    let before = db.query_log().len();
    m.run_ab_test(&db, &db, true);
    assert_eq!(
        db.query_log().len(),
        before + 2,
        "only non-known case runs again"
    );
    assert!(!m.exist_cases("b")[1].pass, "known case pass untouched");

    let before = db.query_log().len();
    m.run_ab_test(&db, &db, false);
    assert_eq!(
        db.query_log().len(),
        before,
        "passed cases skipped without recheck"
    );

    m.append_case(
        "esc",
        Case {
            sql: "SELECT 1 < 2".into(),
            args: None,
            pass: false,
            known: true,
            comment: String::new(),
        },
    );
    m.save().unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    assert!(
        raw.contains("\\u003c"),
        "Go json EscapeHTML must encode < as \\u003c, got: {raw}"
    );

    let _ = fs::remove_file(&path);
}

/// Error: Open missing file; one-sided / both-sided errors; Rows.Err; mismatch.
// `contract_error_paths` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_error_paths() {
    ensure_init();
    let missing = tmp_path("missing");
    let _ = fs::remove_file(&missing);
    assert!(open(missing.to_string_lossy()).is_err());

    let path = tmp_path("errs");
    let m = Manager::new_empty(path.to_string_lossy());
    m.append_case(
        "e",
        Case {
            sql: "SELECT boom".into(),
            args: None,
            pass: true,
            known: false,
            comment: String::new(),
        },
    );

    let ok = Db::always_ok(QueryOutcome {
        columns: vec!["c".into()],
        rows: vec![vec![Some("1".into())]],
        rows_err: None,
    });
    let err = Db::always_err(SqlError::new("query failed"));

    m.run_ab_test(&ok, &err, true);
    assert!(!m.exist_cases("e")[0].pass, "one-sided error must fail");
    assert!(
        Global
            .records()
            .iter()
            .any(|r| r.msg == "One of the result is error"),
        "must log one-sided error"
    );

    m.run_ab_test(&err, &err, true);
    assert!(m.exist_cases("e")[0].pass, "both sides error must pass");

    let rows_err_db = Db::always_ok(QueryOutcome {
        columns: vec!["c".into()],
        rows: vec![],
        rows_err: Some(SqlError::new("cot domain error")),
    });
    m.append_case(
        "e",
        Case {
            sql: "SELECT COT(0)".into(),
            args: None,
            pass: false,
            known: false,
            comment: String::new(),
        },
    );
    m.run_ab_test(&rows_err_db, &ok, true);
    assert!(
        !m.exist_cases("e")[1].pass,
        "Rows.Err on one side must fail the case"
    );

    let db_a = Db::always_ok(QueryOutcome {
        columns: vec!["c".into()],
        rows: vec![vec![Some("1".into())]],
        rows_err: None,
    });
    let db_b = Db::always_ok(QueryOutcome {
        columns: vec!["c".into()],
        rows: vec![vec![Some("2".into())]],
        rows_err: None,
    });
    m.append_case(
        "e",
        Case {
            sql: "SELECT x".into(),
            args: None,
            pass: false,
            known: false,
            comment: String::new(),
        },
    );
    m.run_ab_test(&db_a, &db_b, true);
    assert!(!m.exist_cases("e")[2].pass);
    assert!(Global.records().iter().any(|r| r.msg == "Different result"));

    let _ = fs::remove_file(&path);
}

/// Resource cleanup: DbRows.Close; Save then re-Open.
// `contract_resource_cleanup` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_resource_cleanup() {
    ensure_init();
    let path = tmp_path("cleanup");
    let m = Manager::new_empty(path.to_string_lossy());
    m.append_case(
        "c",
        Case {
            sql: "SELECT 1".into(),
            args: None,
            pass: false,
            known: false,
            comment: String::new(),
        },
    );

    let db = Db::with_handler(move |_q, _a| {
        Ok(QueryOutcome {
            columns: vec!["c".into()],
            rows: vec![vec![Some("1".into())]],
            rows_err: None,
        })
    });

    let mut rows = db.query("SELECT 1", &[]).expect("query ok");
    assert!(!rows.is_closed());
    rows.close().unwrap();
    assert!(rows.is_closed(), "Close must mark rows closed");

    m.run_ab_test(&db, &db, true);
    assert!(m.exist_cases("c")[0].pass);

    m.save().unwrap();
    let m2 = open(path.to_string_lossy()).unwrap();
    assert_eq!(m2.exist_cases("c").len(), 1);

    let _ = fs::remove_file(&path);
}

#[test]
fn json_rejects_incomplete_fraction_like_go_encoding_json() {
    assert!(
        json::unmarshal(br#"{"value":1.}"#).is_err(),
        "encoding/json rejects a fraction without digits after the decimal point"
    );
}

#[test]
fn json_decodes_utf16_surrogate_pairs_like_go_encoding_json() {
    assert_eq!(
        json::unmarshal(br#""\ud83d\ude00""#).unwrap(),
        AnyValue::String("😀".to_string())
    );
}

#[test]
fn json_rejects_unescaped_control_characters_like_go_encoding_json() {
    assert!(json::unmarshal(b"\"line\nbreak\"").is_err());
}

#[test]
fn json_rejects_non_json_unicode_whitespace_like_go_encoding_json() {
    assert!(json::unmarshal("[\u{00a0}null]".as_bytes()).is_err());
}

#[test]
fn json_replaces_invalid_utf8_like_go_encoding_json() {
    assert_eq!(
        json::unmarshal(b"\"\xff\"").unwrap(),
        AnyValue::String("\u{fffd}".to_string())
    );
}

#[test]
fn duplicate_case_fields_use_the_last_value_like_go_encoding_json() {
    let path = tmp_path("duplicate-field");
    fs::write(
        &path,
        r#"{"g":[{"sql":"first","sql":"second","args":null,"pass":false,"known":false,"comment":""}]}"#,
    )
    .unwrap();

    let manager = open(path.to_string_lossy()).unwrap();
    assert_eq!(manager.exist_cases("g")[0].sql, "second");

    let _ = fs::remove_file(path);
}

#[test]
fn case_fields_match_ascii_case_insensitively_like_go_encoding_json() {
    let path = tmp_path("case-insensitive-field");
    fs::write(
        &path,
        r#"{"g":[{"SQL":"upper","ARGS":null,"PASS":true,"KNOWN":false,"COMMENT":"matched"}]}"#,
    )
    .unwrap();

    let manager = open(path.to_string_lossy()).unwrap();
    let case = &manager.exist_cases("g")[0];
    assert_eq!(case.sql, "upper");
    assert!(case.pass);
    assert_eq!(case.comment, "matched");

    let _ = fs::remove_file(path);
}

#[test]
fn json_escapes_line_separator_characters_like_go_encoding_json() {
    assert_eq!(
        json::marshal(&AnyValue::String("\u{2028}\u{2029}".to_string())),
        r#""\u2028\u2029""#
    );
}

#[test]
fn json_uses_short_backspace_and_form_feed_escapes_like_go_encoding_json() {
    assert_eq!(
        json::marshal(&AnyValue::String("\u{0008}\u{000c}".to_string())),
        r#""\b\f""#
    );
}

#[test]
fn rows_scan_rejects_destination_count_mismatch_like_database_sql() {
    let db = Db::always_ok(QueryOutcome {
        columns: vec!["only-column".to_string()],
        rows: vec![vec![Some("one".to_string()), Some("two".to_string())]],
        rows_err: None,
    });
    let mut rows = db.query("SELECT malformed", &[]).unwrap();

    assert!(rows.next());
    assert!(rows.scan_null_strings().is_err());
}

#[test]
fn rows_error_is_reported_after_available_rows_like_database_sql() {
    let db = Db::always_ok(QueryOutcome {
        columns: vec!["value".to_string()],
        rows: vec![vec![Some("available".to_string())]],
        rows_err: Some(SqlError::new("iteration failed")),
    });
    let mut rows = db.query("SELECT partial", &[]).unwrap();

    assert!(
        rows.next(),
        "available rows must be yielded before Rows.Err"
    );
    assert_eq!(
        rows.scan_null_strings().unwrap(),
        vec![Some("available".to_string())]
    );
    assert!(!rows.next());
    assert_eq!(rows.err().unwrap().message, "iteration failed");
}
