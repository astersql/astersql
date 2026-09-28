// Copyright 2026 AsterSQL.

//! Parity tests for `tests/llmtest` public contracts vs Go `main.go`.

// 本文件对应 `tests/llmtest/parity_test.rs`，本次任务只补中文解释，不改行为。
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
use crate::main::{create_generate_cmd, create_verify_cmd, unify_dsn};
use crate::stubs::{
    ExitCalled, SqlDb, clear_opened_dbs, clear_sql_open_handler, opened_dbs, set_capture_exit,
    set_sql_open_handler,
};
use astersql_tests_llmtest_generator as generator;
use astersql_tests_llmtest_logger::{Global, ensure_init as ensure_logger};
use astersql_tests_llmtest_testcase::SqlError;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

// `TEST_LOCK` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static TEST_LOCK: Mutex<()> = Mutex::new(());

// `lock_tests` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn lock_tests() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// `tmp_dir` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn tmp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("llmtest-main-{label}-{nanos}"));
    fs::create_dir_all(dir.join("testdata")).unwrap();
    dir
}

// `assert_exit1` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn assert_exit1(result: std::thread::Result<()>) {
    match result {
        Ok(()) => panic!("expected os.Exit(1), catch_unwind returned Ok"),
        Err(payload) => {
            let code = payload
                .downcast_ref::<ExitCalled>()
                .unwrap_or_else(|| panic!("expected ExitCalled(1), got other panic"))
                .0;
            assert_eq!(code, 1);
        }
    }
}

// 测试 `go_rust_public_contract_matches` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
#[test]
// `go_rust_public_contract_matches` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn go_rust_public_contract_matches() {
    let _g = lock_tests();
    set_capture_exit(true);
    clear_sql_open_handler();
    clear_opened_dbs();
    ensure_logger();
    generator::ensure_init();

    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();

    set_capture_exit(false);
    clear_sql_open_handler();
}

/// Normal: unifyDSN rewrites collation; generate/verify command shapes; generate with count=0.
// `contract_normal_paths` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_normal_paths() {
    let out = unify_dsn("user:pass@tcp(127.0.0.1:4000)/test");
    assert!(
        out.contains("collation=utf8mb4_bin"),
        "unifyDSN must force utf8mb4_bin, got {out}"
    );
    assert!(
        out.contains("tcp(127.0.0.1:4000)"),
        "addr must round-trip, got {out}"
    );
    assert!(
        out.contains("user:pass@"),
        "user/pass must round-trip, got {out}"
    );
    assert!(out.contains("/test"), "dbname must round-trip, got {out}");

    let out2 = unify_dsn("root@tcp(127.0.0.1:3306)/db?collation=utf8mb4_general_ci");
    assert!(
        out2.contains("collation=utf8mb4_bin"),
        "must override collation, got {out2}"
    );
    assert!(
        !out2.contains("utf8mb4_general_ci"),
        "old collation must not remain, got {out2}"
    );

    let generate_cmd = create_generate_cmd();
    assert_eq!(generate_cmd.use_name(), "generate");
    assert_eq!(generate_cmd.short(), "Generate something using OpenAI");

    let ver = create_verify_cmd();
    assert_eq!(ver.use_name(), "verify");
    assert_eq!(ver.short(), "Verify something using TiDB and MySQL");

    let dir = tmp_dir("normal");
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();
    fs::write(
        dir.join("testdata/dml.json"),
        r#"{"insert":[{"sql":"SELECT 1","args":null,"pass":false,"known":false,"comment":""}]}"#,
    )
    .unwrap();

    let mut root = crate::stubs::cobra::Command::new("llmtest");
    root.add_command(create_generate_cmd());
    root.add_command(create_verify_cmd());
    root.execute_args(&[
        "generate".into(),
        "--prompt_generator".into(),
        "dml".into(),
        "--test_count".into(),
        "0".into(),
        "--parallel".into(),
        "2".into(),
    ])
    .expect("generate should succeed");

    let saved = fs::read_to_string(dir.join("testdata/dml.json")).unwrap();
    assert!(
        saved.contains("SELECT 1"),
        "Save must rewrite testdata, got {saved}"
    );

    std::env::set_current_dir(prev).unwrap();
    let _ = fs::remove_dir_all(&dir);
}

/// Boundary: empty DSN still parses (Go allows len==0); command names stable.
// `contract_boundary` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_boundary() {
    let out = unify_dsn("");
    assert!(
        out.contains("collation=utf8mb4_bin"),
        "empty DSN still gets collation, got {out}"
    );
    assert!(
        out.contains("tcp(127.0.0.1:3306)"),
        "empty DSN uses Go defaults, got {out}"
    );

    assert_eq!(create_generate_cmd().use_name(), "generate");
    assert_eq!(create_verify_cmd().child_names().len(), 0);
}

#[test]
fn unify_dsn_preserves_go_mysql_escaping_and_ipv6() {
    let out = unify_dsn("user@tcp(::1)/db%2Fname?custom=hello+world");
    assert_eq!(
        out,
        "user@tcp([::1]:3306)/db%2Fname?collation=utf8mb4_bin&custom=hello+world"
    );
}

#[test]
fn cobra_dispatch_and_bool_flags_match_go() {
    let _g = lock_tests();
    clear_sql_open_handler();
    clear_opened_dbs();
    ensure_logger();
    let mut root = crate::stubs::cobra::Command::new("llmtest");
    root.add_command(create_verify_cmd());

    let err = root
        .execute_args(&["does-not-exist".into()])
        .expect_err("Cobra rejects an unknown subcommand");
    assert!(err.contains("unknown command"), "unexpected error: {err}");

    let dir = tmp_dir("bool-short-form");
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();
    fs::write(dir.join("testdata/misc.json"), r#"{"cte":[]}"#).unwrap();

    root.execute_args(&[
        "verify".into(),
        "--prompt_generator=misc".into(),
        "--tidb_dsn=u@tcp(127.0.0.1:4000)/t".into(),
        "--mysql_dsn=u@tcp(127.0.0.1:3306)/t".into(),
        "--recheck_passed=t".into(),
    ])
    .expect("Go pflag accepts strconv.ParseBool short forms");

    std::env::set_current_dir(prev).unwrap();
    let _ = fs::remove_dir_all(dir);
}

/// Error: bad DSN / unknown generator / open failures → log + os.Exit(1).
// `contract_error_paths` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_error_paths() {
    ensure_logger();
    let before_dsn = Global.records().len();

    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = unify_dsn("no-slash-dsn");
    }));
    assert_exit1(result);
    assert!(
        Global.records()[before_dsn..]
            .iter()
            .any(|r| r.level == "error" && r.msg == "Failed to parse DSN"),
        "must log Failed to parse DSN"
    );

    let before_unk = Global.records().len();
    let mut root = crate::stubs::cobra::Command::new("llmtest");
    root.add_command(create_generate_cmd());
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = root.execute_args(&[
            "generate".into(),
            "--prompt_generator".into(),
            "does-not-exist".into(),
        ]);
    }));
    assert_exit1(result);
    assert!(
        Global.records()[before_unk..]
            .iter()
            .any(|r| r.level == "info" && r.msg == "Unknown prompt generator"),
        "must log Unknown prompt generator"
    );

    // Use an empty cwd so package-local testdata/dml.json is not picked up.
    let dir = tmp_dir("missing-case");
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();
    // testdata/ exists but dml.json does not.
    let before_open = Global.records().len();
    let mut root = crate::stubs::cobra::Command::new("llmtest");
    root.add_command(create_generate_cmd());
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = root.execute_args(&[
            "generate".into(),
            "--prompt_generator".into(),
            "dml".into(),
            "--test_count".into(),
            "0".into(),
        ]);
    }));
    assert_exit1(result);
    assert!(
        Global.records()[before_open..]
            .iter()
            .any(|r| r.level == "error" && r.msg == "Failed to open test case"),
        "must log Failed to open test case"
    );
    std::env::set_current_dir(&prev).unwrap();
    let _ = fs::remove_dir_all(&dir);

    let before_tidb = Global.records().len();
    clear_opened_dbs();
    set_sql_open_handler(|_driver: &str, _dsn: &str| -> Result<SqlDb, SqlError> {
        Err(SqlError::new("dial refused"))
    });
    let dir = tmp_dir("err-open");
    std::env::set_current_dir(&dir).unwrap();
    fs::write(dir.join("testdata/misc.json"), r#"{"cte":[]}"#).unwrap();

    let mut root = crate::stubs::cobra::Command::new("llmtest");
    root.add_command(create_verify_cmd());
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = root.execute_args(&[
            "verify".into(),
            "--prompt_generator".into(),
            "misc".into(),
            "--tidb_dsn".into(),
            "root@tcp(127.0.0.1:4000)/test".into(),
            "--mysql_dsn".into(),
            "root@tcp(127.0.0.1:3306)/test".into(),
        ]);
    }));
    assert_exit1(result);
    assert!(
        Global.records()[before_tidb..]
            .iter()
            .any(|r| r.level == "error" && r.msg == "Failed to open TiDB"),
        "must log Failed to open TiDB"
    );

    std::env::set_current_dir(prev).unwrap();
    let _ = fs::remove_dir_all(&dir);
    clear_sql_open_handler();
}

/// Resource cleanup: verify defers Close on TiDB only (not MySQL).
// `contract_resource_cleanup` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_resource_cleanup() {
    ensure_logger();
    clear_opened_dbs();
    clear_sql_open_handler();

    let dir = tmp_dir("cleanup");
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();
    fs::write(
        dir.join("testdata/misc.json"),
        r#"{"cte":[{"sql":"SELECT 1","args":null,"pass":true,"known":false,"comment":""}]}"#,
    )
    .unwrap();

    let mut root = crate::stubs::cobra::Command::new("llmtest");
    root.add_command(create_verify_cmd());
    root.execute_args(&[
        "verify".into(),
        "--prompt_generator".into(),
        "misc".into(),
        "--tidb_dsn".into(),
        "u@tcp(127.0.0.1:4000)/t".into(),
        "--mysql_dsn".into(),
        "u@tcp(127.0.0.1:3306)/t".into(),
        "--recheck_passed".into(),
    ])
    .expect("verify should succeed");

    let opened = opened_dbs();
    assert_eq!(opened.len(), 2, "TiDB + MySQL opens");
    assert!(
        opened[0].1.contains("collation=utf8mb4_bin"),
        "TiDB DSN unified: {}",
        opened[0].1
    );
    assert!(
        opened[1].1.contains("collation=utf8mb4_bin"),
        "MySQL DSN unified: {}",
        opened[1].1
    );
    assert!(opened[0].2.is_closed(), "Go defer tidb.Close must run");
    assert!(!opened[1].2.is_closed(), "Go does not defer mysql.Close");

    std::env::set_current_dir(prev).unwrap();
    let _ = fs::remove_dir_all(&dir);
}
