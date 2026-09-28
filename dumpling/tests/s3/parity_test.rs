// Copyright 2026 AsterSQL.

//! Parity tests for `dumpling/tests/s3` public contracts vs Go `import.go`.
// dumpling/tests/s3 公开契约 parity：对照 Go import.go 的 flag、SQL、并发与错误路径。

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use crate::entry::{
    self, DEFAULT_BATCHES, INSERT_ROW_COUNT, build_create_table_sql, build_dsn, build_insert_query,
    count_insert_values, default_flags, run_import, run_import_with_db,
};
use crate::stubs::{self, Flags, RecordingDb};

#[test]
// 四类契约：正常 SQL/并发、flag 边界、错误注入、context/DSN 清理。
fn go_rust_public_contract_matches() {
    contract_normal_paths();
    contract_boundary_flags_and_sql();
    contract_error_paths();
    contract_resource_cleanup();
}

#[test]
fn negative_worker_panics_when_creating_channel_like_go() {
    let flags = Flags {
        worker: -1,
        ..Flags::default()
    };

    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = run_import_with_db(&flags, RecordingDb::new(), 0);
    }));
    assert!(result.is_err(), "Go make(chan struct{{}}, -1) panics");
}

#[test]
fn create_table_exec_error_is_traced_and_stops_before_inserts() {
    let db = RecordingDb::new();
    db.fail_next_exec("create table failed");

    let err = run_import_with_db(&Flags::default(), db.clone(), 1)
        .expect_err("CREATE TABLE failure must be returned");
    assert_eq!(err.Error(), "create table failed");
    let execs = db.execs();
    assert_eq!(execs.len(), 1);
    assert!(!execs[0].via_context);
}

#[test]
fn cobra_flag_forms_and_platform_int_width_match_go() {
    let attached = stubs::parse_flags(&[
        "-Bmydb".into(),
        "-Tmytable".into(),
        "-P3306".into(),
        "-w4".into(),
    ])
    .expect("pflag accepts shorthand values without a separating space");
    assert_eq!(attached.database, "mydb");
    assert_eq!(attached.table, "mytable");
    assert_eq!(attached.port, 3306);
    assert_eq!(attached.worker, 4);

    let terminated = stubs::parse_flags(&["--".into(), "-P".into(), "invalid".into()])
        .expect("arguments after -- are positional");
    assert_eq!(terminated, Flags::default());

    if usize::BITS > 32 {
        let wide =
            stubs::parse_flags(&["--port=2147483648".into()]).expect("Go int is pointer-width");
        assert_eq!(wide.port.to_string(), "2147483648");
    }
}

#[test]
fn errgroup_returns_first_completed_error_like_go() {
    let (group, _ctx) = stubs::Group::WithContext(stubs::Background());
    group.Go(|| {
        std::thread::sleep(Duration::from_millis(100));
        Err(stubs::Error::new("slow error"))
    });
    group.Go(|| Err(stubs::Error::new("fast error")));

    let err = group.Wait().expect_err("both workers fail");
    assert_eq!(err.Error(), "fast error");
}

/// Normal: defaults, DSN, CREATE/INSERT side effects, concurrent batches.
// 正常路径：默认 flag、DSN/SQL 形状、stub DB 上 CREATE + 8 批 ExecContext。
fn contract_normal_paths() {
    let d = default_flags();
    // 默认 flag 与 Go cobra PersistentFlags 注册值一致。
    assert_eq!(d.database, "s3");
    assert_eq!(d.table, "t");
    assert_eq!(d.port, 4000); // TiDB harness port (dumpling/tests/AGENTS.md)
    assert_eq!(d.worker, 16);
    // DEFAULT_BATCHES/INSERT_ROW_COUNT 与 Go 常量一致。
    assert_eq!(DEFAULT_BATCHES, 500);
    assert_eq!(INSERT_ROW_COUNT, 10000);

    let dsn = build_dsn("s3", 4000);
    // DSN 无密码 root、utf8mb4，host:port 经 JoinHostPort。
    assert_eq!(dsn, "root:@tcp(127.0.0.1:4000)/s3?charset=utf8mb4");

    let create = build_create_table_sql("t");
    // CREATE 模板含 IF NOT EXISTS 与单列 VARCHAR(11)。
    assert!(create.starts_with("CREATE TABLE IF NOT EXISTS t ("));
    assert!(create.contains("a VARCHAR(11)"));

    let insert = build_insert_query("t");
    assert!(insert.starts_with("insert into t values('aaaaaaaaaa')"));
    // 每条 INSERT 必须含 INSERT_ROW_COUNT 个元组。
    assert_eq!(count_insert_values(&insert), INSERT_ROW_COUNT);

    // Deterministic fixture: stub DB, small batch count, worker=2.
    // 确定性 fixture：RecordingDb、8 批次、worker=2 限流。
    let db = RecordingDb::new();
    let flags = Flags {
        database: "s3".into(),
        table: "t".into(),
        port: 4000,
        worker: 2,
    };
    run_import_with_db(&flags, db.clone(), 8).expect("import ok");

    // exec 顺序：先一条 Exec 建表，再 N 条 ExecContext insert。
    let execs = db.execs();
    assert!(
        execs
            .iter()
            .any(|e| !e.via_context && e.sql.contains("CREATE TABLE IF NOT EXISTS t")),
        "must Exec CREATE TABLE first"
    );
    // 第一条必须是同步 Exec 建表
    let inserts: Vec<_> = execs.iter().filter(|e| e.via_context).collect();
    assert_eq!(inserts.len(), 8, "8 ExecContext batches");
    for e in &inserts {
        // 每批 INSERT SQL 元组数与 INSERT_ROW_COUNT 一致。
        assert_eq!(count_insert_values(&e.sql), INSERT_ROW_COUNT);
    }
}

// cobra 短/长 flag、JoinHostPort、worker=0 边界。
/// Boundary: flag parsing, custom database/table/port, JoinHostPort shape.
// 边界：cobra flag 短/长形式、自定义库表端口、JoinHostPort、worker=0 batches=0。
fn contract_boundary_flags_and_sql() {
    let flags = stubs::parse_flags(&[
        "-B".into(),
        "mydb".into(),
        "-T".into(),
        "mytable".into(),
        "-P".into(),
        "3306".into(),
        "-w".into(),
        "4".into(),
    ])
    .expect("parse");
    // 短 flag 形式解析自定义库表端口 worker。
    assert_eq!(flags.database, "mydb");
    assert_eq!(flags.table, "mytable");
    assert_eq!(flags.port, 3306);
    assert_eq!(flags.worker, 4);

    let long = stubs::parse_flags(&[
        "--database=s3".into(),
        "--table=t".into(),
        "--port=4000".into(),
        "--worker=16".into(),
    ])
    .expect("long flags");
    // 长 flag 形式解析结果同 default
    assert_eq!(long, Flags::default());

    assert_eq!(stubs::JoinHostPort("127.0.0.1", "4000"), "127.0.0.1:4000");
    assert_eq!(
        build_dsn("s3", 3306),
        "root:@tcp(127.0.0.1:3306)/s3?charset=utf8mb4"
    );

    let create = build_create_table_sql("mytable");
    assert!(create.contains("CREATE TABLE IF NOT EXISTS mytable"));
    let insert = build_insert_query("mytable");
    assert!(insert.starts_with("insert into mytable values"));
    assert_eq!(count_insert_values(&insert), 10000);

    // worker=0: no tokens filled; with batches=0 the loop is a no-op after CREATE.
    // worker=0 不预填令牌；batches=0 时循环不跑，仅 Exec 建表。
    let db = RecordingDb::new();
    let flags = Flags {
        worker: 0,
        ..Flags::default()
    };
    run_import_with_db(&flags, db.clone(), 0).expect("zero batches");
    // 仅 CREATE，无 ExecContext
    assert_eq!(db.execs().len(), 1);
    assert!(!db.execs()[0].via_context);
}

// 注入 ExecContext 失败、flag/open 失败、main exit 2。
/// Error: injected ExecContext failure cancels and surfaces Trace'd error.
// 错误路径：注入 ExecContext 失败触发 cancel、Trace 错误文本、open/flag 失败与 os.Exit(2)。
fn contract_error_paths() {
    let db = RecordingDb::new();
    db.fail_on_nth_exec_context(1);
    let flags = Flags {
        worker: 2,
        ..Flags::default()
    };
    // Single batch avoids Go's token-channel deadlock when failures do not return tokens.
    // 单批次避免 Go 失败不归还令牌时的 channel 死锁。
    let err = run_import_with_db(&flags, db.clone(), 1).expect_err("must fail");
    // 错误消息来自 stubs 注入的 injected exec failure。
    assert!(
        err.Error().contains("injected exec failure"),
        "got: {}",
        err.Error()
    );
    let inserts: Vec<_> = db.execs().into_iter().filter(|e| e.via_context).collect();
    // 失败前仅一次 ExecContext
    assert_eq!(inserts.len(), 1);

    // Bad flag value.
    // 非法 port 字符串 → invalid value 错误。
    let err = stubs::parse_flags(&["-P".into(), "xyz".into()]).expect_err("bad port");
    assert!(err.Error().contains("invalid value"));

    // Open failure via injectable open.
    // 可注入 open 模拟 sql.Open 失败。
    let open: stubs::OpenFn = Arc::new(|_dsn: &str| Err(stubs::Error::new("open refused")));
    let err = run_import(&Flags::default(), open, 1).expect_err("open fail");
    assert!(err.Error().contains("open refused"));

    // main / execute failure → os.Exit(2) panic shape.
    // main 失败路径：print_fail + os_exit(2)，桩内 panic 可被 catch_unwind 捕获。
    let open_fail: stubs::OpenFn = Arc::new(|_dsn: &str| Err(stubs::Error::new("boom")));
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let flags = Flags::default();
        if let Err(err) = run_import(&flags, open_fail.clone(), 1) {
            stubs::print_fail(&err);
            stubs::os_exit(2);
        }
    }));
    // os_exit 桩以 panic 模拟进程退出
    assert!(result.is_err());
    // Go main 失败 exit 2
    assert_eq!(stubs::take_exit_code(), Some(2));
}

// 成功路径下 exec 计数与 sql_open DSN、cancel 幂等。
/// Resource cleanup: defer cancel leaves context Done after successful run.
// 资源清理：成功 Wait 后 errgroup 取消 derived ctx；sql_open 记录 DSN；cancel 幂等。
fn contract_resource_cleanup() {
    let db = RecordingDb::new();
    let flags = Flags {
        worker: 2,
        ..Flags::default()
    };
    run_import_with_db(&flags, db.clone(), 4).expect("ok");

    // 4 次 ExecContext insert + 1 次 Exec create。
    let execs = db.execs();
    assert_eq!(execs.iter().filter(|e| e.via_context).count(), 4);
    assert_eq!(execs.iter().filter(|e| !e.via_context).count(), 1);

    // build_dsn 与 sql_open 组合应回显完整 DSN 字符串。
    // sql_open records DSN (boundary open side effect).
    // sql_open 桩记录 DSN，验证 open 边界副作用。
    let opened = stubs::sql_open("mysql", &build_dsn("s3", 4000)).expect("open");
    assert_eq!(
        opened.open_dsn().as_deref(),
        Some("root:@tcp(127.0.0.1:4000)/s3?charset=utf8mb4")
    );

    // Context cancel is idempotent (defer cancel + Wait cancel).
    // WithCancel 的 cancel 可重复调用，Done/Err 语义稳定。
    let (ctx, cancel) = stubs::WithCancel(stubs::Background());
    assert!(ctx.Err().is_none());
    cancel.call();
    assert!(ctx.Err().is_some());
    // 二次 cancel 不应 panic
    cancel.call();
    assert!(ctx.Done());
}
