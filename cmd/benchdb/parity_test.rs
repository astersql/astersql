// Copyright 2026 AsterSQL.

//! Parity tests for `cmd/benchdb` public contracts vs Go `main.go`.
//!
//! 这组测试不是验证单一函数的局部实现，而是把 Rust 端暴露给命令行入口的
//! 行为当作一个整体契约来锁定，确保它与 Go `main.go` 的公开语义保持一致。
//! 关注点包括默认参数、作业分发顺序、SQL 模板、错误处理和资源释放路径。
//! 因为 `benchdb` 本身是命令型工具，这里优先验证“外部能观察到什么”，
//! 而不是把内部细节拆成大量脆弱的小断言。

use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;

use crate::entry::{self, BenchDB, c_log, default_flags, new_bench_db, run_with_flags};
use crate::stubs::{
    self, Flags, RecordingSession, SessionFactory, SqlArg, StoreType, default_run_jobs,
};

#[test]
fn go_rust_public_contract_matches() {
    // 聚合四类子契约，和 Go 命令的职责分层对应：
    // 正常路径、边界解析、致命错误、资源清理。
    // 这样一旦某类公开行为发生回归，失败位置会直接暴露是哪一层契约失真。
    contract_normal_jobs_and_sql_side_effects();
    contract_boundary_parse_and_batch();
    contract_error_paths();
    contract_resource_cleanup();
}

#[test]
fn negative_count_matches_go_integer_range_semantics() {
    let mut ut = new_bench_db(&Flags::default(), &SessionFactory::default());
    let mut called = false;

    // Go's `for range count` performs zero iterations for a negative integer,
    // then `0 / time.Duration(count)` yields a zero duration without panicking.
    ut.run_count_times("negative", -1, |_| called = true);

    assert!(
        !called,
        "negative count must not execute the benchmark body"
    );
}

#[test]
fn lone_dash_stops_flag_parsing_like_go() {
    // The Go flag package treats any argument shorter than two bytes, including
    // a lone dash, as the first positional argument and stops parsing there.
    let flags = stubs::parse_flags(&["-".into(), "-batch=7".into()]);

    assert_eq!(flags.batch_size, 100);
}

#[test]
fn help_flag_exits_successfully_without_running_jobs() {
    const CHILD_ENV: &str = "ASTERSQL_BENCHDB_HELP_TEST_CHILD";
    if std::env::var_os(CHILD_ENV).is_some() {
        let _ = stubs::parse_flags(&["-h".into()]);
        panic!("help parsing returned and would continue into benchmark jobs");
    }

    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "parity_test::help_flag_exits_successfully_without_running_jobs",
        ])
        .env(CHILD_ENV, "1")
        .status()
        .unwrap();

    assert!(
        status.success(),
        "Go flag help exits the process with status 0"
    );
}

/// Normal path: defaults, job dispatch, SQL templates and args match Go.
/// 覆盖最常见的成功路径，确认 Rust 入口对外表现与 Go 主流程一致。
/// 这里既检查默认 flag，也检查 `run` 作业串如何驱动具体 SQL 副作用。
/// 断言重点放在用户可见的合同上，而不是存根内部的临时状态。
fn contract_normal_jobs_and_sql_side_effects() {
    // 先模拟 `create|truncate` 两个标准作业串联执行。
    // 这对应 Go `main()` 中按 `runJobs` 顺序逐个分派 work 的主路径。
    let flags = Flags {
        run_jobs: "create|truncate".to_string(),
        table_name: "benchdb".to_string(),
        ..Flags::default()
    };
    let session = RecordingSession::new();
    let session_for_factory = session.clone();
    let factory = SessionFactory {
        create: Rc::new(move |_store| (session_for_factory.clone(), None)),
    };
    run_with_flags(flags, factory);

    // `newBenchDB()` 启动后会先执行 `use test`，这是 Go 版本 bootstrap 的对外副作用。
    // 如果这一步缺失，后续建表和 DML 的数据库上下文就不再与 Go 对齐。
    let execs = session.execs();
    // bootstrap "use test" + create + truncate
    assert!(
        execs.iter().any(|e| e.sql == "use test"),
        "bootstrap must ExecuteInternal use test"
    );
    assert!(
        execs
            .iter()
            .any(|e| e.sql.contains("CREATE TABLE IF NOT EXISTS %n")),
        "createTable SQL template"
    );
    let create = execs
        .iter()
        .find(|e| e.sql.contains("CREATE TABLE IF NOT EXISTS %n"))
        .unwrap();
    assert_eq!(create.args, vec![SqlArg::Ident("benchdb".into())]);

    let trunc = execs.iter().find(|e| e.sql == "truncate table %n").unwrap();
    assert_eq!(trunc.args, vec![SqlArg::Ident("benchdb".into())]);

    // 默认参数是命令行合同的一部分。
    // 这里直接锁定 Go `flag` 默认值，避免 Rust 端无意改掉工具的开箱行为。
    // Default flag contract
    let d = default_flags();
    assert_eq!(d.addr, "127.0.0.1:2379");
    assert_eq!(d.table_name, "benchdb");
    assert_eq!(d.batch_size, 100);
    assert_eq!(d.blob_size, 1000);
    assert_eq!(d.log_level, "warn");
    assert_eq!(d.run_jobs, default_run_jobs());
    assert!(d.run_jobs.contains("gc"));
    assert!(d.run_jobs.contains("update-random:0_10000:100000"));

    // Go 版本创建 TiKV store 时会显式关闭 GC，并把全局 store 类型设置成 TiKV。
    // 这里验证的是初始化副作用，而不是具体通过哪个内部 API 达成。
    // Store path disables GC like Go.
    let ut = new_bench_db(&Flags::default(), &SessionFactory::default());
    assert!(ut.store.disable_gc);
    assert!(ut.store.path.starts_with("tikv://127.0.0.1:2379"));
    assert_eq!(stubs::get_global_config().store, StoreType::TiKV);

    // 下面这组断言覆盖主要 work 解析和对应 SQL 发射行为。
    // 它们共同证明 Rust 端对 `insert / update / select / query` 的对外契约
    // 和 Go `switch` 分派到各个方法后的效果是一致的。
    // mustParseWork / insert / update-random / select / query
    let session = RecordingSession::new();
    let mut ut = BenchDB {
        store: ut.store.clone(),
        session: session.clone(),
        flags: Flags {
            batch_size: 2,
            blob_size: 4,
            table_name: "t1".into(),
            ..Flags::default()
        },
    };
    assert_eq!(
        ut.must_parse_work("insert:0_10"),
        ("insert".into(), "0_10".into())
    );
    assert_eq!(
        ut.must_parse_work("query:select 1:3"),
        ("query".into(), "select 1:3".into())
    );

    // `insert_rows("0_3")` 在 batch_size=2 时应拆成两次事务。
    // 这直接对应 Go 的 `loopCount = (end-start+batch-1)/batch` 取整逻辑。
    ut.insert_rows("0_3");
    let execs = session.execs();
    let begins = execs.iter().filter(|e| e.sql == "begin").count();
    let commits = execs.iter().filter(|e| e.sql == "commit").count();
    // loopCount = (3-0+2-1)/2 = 2
    assert_eq!(begins, 2);
    assert_eq!(commits, 2);
    let inserts: Vec<_> = execs
        .iter()
        .filter(|e| e.sql.starts_with("insert %n"))
        .collect();
    assert_eq!(inserts.len(), 3);
    assert_eq!(inserts[0].args[0], SqlArg::Ident("t1".into()));
    assert_eq!(inserts[0].args[1], SqlArg::Int(0));
    assert_eq!(inserts[2].args[1], SqlArg::Int(2));
    // Go 版本用 `blobSize/2` 生成随机字节切片。
    // 这里只看长度，不关心随机内容本身，以避免把测试变成脆弱的实现细节比较。
    if let SqlArg::Bytes(b) = &inserts[0].args[3] {
        assert_eq!(b.len(), 2); // blobSize/2
    } else {
        panic!("expected blob bytes");
    }

    // `update-random` 的合同不是具体随机序列，而是更新次数和取值范围。
    // 因此断言每条 SQL 都落在 `[start, end)`，与 Go 的 `rand.Intn(end-start)+start` 对齐。
    let session2 = RecordingSession::new();
    let mut ut2 = BenchDB {
        store: ut.store.clone(),
        session: session2.clone(),
        flags: Flags {
            batch_size: 3,
            table_name: "t1".into(),
            ..Flags::default()
        },
    };
    ut2.update_random_rows("0_10:5");
    let ups: Vec<_> = session2
        .execs()
        .into_iter()
        .filter(|e| {
            e.sql
                .starts_with("update %n set exp = exp + 1 where id = %?")
        })
        .collect();
    assert_eq!(ups.len(), 5);
    for u in &ups {
        if let SqlArg::Int(id) = u.args[1] {
            assert!((0..10).contains(&id));
        } else {
            panic!("id arg");
        }
    }

    // `update-range` 没有随机性，适合直接锁定模板和参数顺序。
    // 这能防止 Rust 迁移时把上下界位置颠倒，导致语义悄悄偏离 Go。
    let session3 = RecordingSession::new();
    let mut ut3 = BenchDB {
        store: ut.store.clone(),
        session: session3.clone(),
        flags: Flags {
            table_name: "t1".into(),
            ..Flags::default()
        },
    };
    ut3.update_range_rows("5_10:2");
    let range_ups: Vec<_> = session3
        .execs()
        .iter()
        .filter(|e| e.sql.contains("id >= %? and id < %?"))
        .cloned()
        .collect();
    assert_eq!(range_ups.len(), 2);
    assert_eq!(
        range_ups[0].args,
        vec![SqlArg::Ident("t1".into()), SqlArg::Int(5), SqlArg::Int(10)]
    );

    // `select` 路径只验证执行次数即可，因为 SQL 模板已在存根中被完整记录。
    // 对这类只读操作，次数就是最直接的外部观测值。
    let session4 = RecordingSession::new();
    let mut ut4 = BenchDB {
        store: ut.store.clone(),
        session: session4.clone(),
        flags: Flags {
            table_name: "t1".into(),
            ..Flags::default()
        },
    };
    ut4.select_rows("0_100:3");
    assert_eq!(
        session4
            .execs()
            .iter()
            .filter(|e| e.sql.starts_with("select * from %n"))
            .count(),
        3
    );

    // `query` 保留原始 SQL 文本，重点在于重复执行次数必须遵守 spec 中的计数。
    // 这里锁住的是命令工具的“透传查询”语义。
    let session5 = RecordingSession::new();
    let mut ut5 = BenchDB {
        store: ut.store.clone(),
        session: session5.clone(),
        flags: Flags::default(),
    };
    ut5.query("select 1:4");
    assert_eq!(
        session5
            .execs()
            .iter()
            .filter(|e| e.sql == "select 1")
            .count(),
        4
    );

    // Go `main()` 遇到未知 job 会打印日志并立刻返回，后续 work 不再执行。
    // 默认作业串里带有 `gc`，而 Rust 当前同样保持“未知即停止”的外部行为。
    // Unknown job (including default "gc") stops the pipeline — Go behavior.
    let session6 = RecordingSession::new();
    let session6f = session6.clone();
    run_with_flags(
        Flags {
            run_jobs: "create|gc|truncate".to_string(),
            ..Flags::default()
        },
        SessionFactory {
            create: Rc::new(move |_| (session6f.clone(), None)),
        },
    );
    let sqls: Vec<_> = session6.execs().into_iter().map(|e| e.sql).collect();
    assert!(sqls.iter().any(|s| s.contains("CREATE TABLE")));
    assert!(
        !sqls.iter().any(|s| s == "truncate table %n"),
        "truncate after unknown gc must not run"
    );

    // Go 同时接受连字符和下划线别名。
    // 这里保留该兼容层，避免用户已有脚本在 Rust 端失效。
    // update_random underscore alias
    let (name, _) = ut.must_parse_work("update_random:0_1:1");
    assert_eq!(name, "update_random");
    // 仅引用日志辅助符号，保证这些公开入口在测试构建中持续被链接和暴露。
    let _ = c_log; // keep helper linked for color log contract
    let _ = entry::c_log_f;
}

/// Boundary: ranges, default count, batch ceilings, flag parse.
/// 覆盖最容易在迁移时出现 off-by-one 或解析歧义的边界条件。
/// 这些断言为上层命令行输入格式提供护栏，防止 Rust 和 Go 在小输入上分叉。
fn contract_boundary_parse_and_batch() {
    // 先锁定基础解析器的输入输出。
    // 这几项是所有 job spec 的共同前置条件，一旦漂移会连带破坏多个路径。
    let ut = new_bench_db(&Flags::default(), &SessionFactory::default());
    assert_eq!(ut.must_parse_range("0_0"), (0, 0));
    assert_eq!(ut.must_parse_range("0_1"), (0, 1));
    assert_eq!(ut.must_parse_spec("0_10"), (0, 10, 1));
    assert_eq!(ut.must_parse_spec("0_10:7"), (0, 10, 7));
    assert_eq!(ut.must_parse_int("42"), 42);

    // 即使 batch 远大于待插入行数，也必须至少跑一轮事务。
    // 该断言专门卡住向上取整规则，防止循环次数被错误地算成 0。
    // insert loopCount ceiling: (end-start+batch-1)/batch
    let session = RecordingSession::new();
    let mut ut = BenchDB {
        store: ut.store,
        session: session.clone(),
        flags: Flags {
            batch_size: 100,
            blob_size: 2,
            ..Flags::default()
        },
    };
    ut.insert_rows("0_1");
    // loopCount = (1-0+100-1)/100 = 1; one begin/commit; one insert then id==end break
    assert_eq!(
        session.execs().iter().filter(|e| e.sql == "begin").count(),
        1
    );
    assert_eq!(
        session
            .execs()
            .iter()
            .filter(|e| e.sql.starts_with("insert %n"))
            .count(),
        1
    );

    // 参数解析测试直接对齐 Go `flag` 风格：
    // 支持 `-k v` 与 `-k=v` 混用，避免 Rust CLI 解析器行为更“严格”。
    let f = stubs::parse_flags(&[
        "-addr".into(),
        "9.9.9.9:2379".into(),
        "-table=foo".into(),
        "-batch=5".into(),
        "-blob".into(),
        "8".into(),
        "-L=info".into(),
        "-run=create".into(),
    ]);
    assert_eq!(f.addr, "9.9.9.9:2379");
    assert_eq!(f.table_name, "foo");
    assert_eq!(f.batch_size, 5);
    assert_eq!(f.blob_size, 8);
    assert_eq!(f.log_level, "info");
    assert_eq!(f.run_jobs, "create");

    // Go's flag package stops parsing at the first positional argument and at
    // an explicit `--`; later flag-shaped arguments remain positional.
    let f = stubs::parse_flags(&["payload".into(), "-table=must-not-be-parsed".into()]);
    assert_eq!(f.table_name, "benchdb");

    let f = stubs::parse_flags(&["--".into(), "-batch=7".into()]);
    assert_eq!(f.batch_size, 100);

    // Go `int` is 64-bit on the supported target, so valid values must not be
    // rejected merely because they exceed the Rust `i32` range.
    let f = stubs::parse_flags(&["-batch=2147483648".into(), "-blob=2147483649".into()]);
    assert_eq!(f.batch_size, 2_147_483_648);
    assert_eq!(f.blob_size, 2_147_483_649);
}

/// Error paths: invalid range / int / unknown flag → fatal (panic).
/// Go 版本使用 `log.Fatal` 终止流程；Rust 存根以 `panic` 模拟同一合同。
/// 因此这里不比较错误类型体系，而是验证这些非法输入一定走到致命路径。
fn contract_error_paths() {
    let ut = new_bench_db(&Flags::default(), &SessionFactory::default());

    // 缺少下划线分隔符时，范围字符串无法解析。
    // 错误消息需要包含 Go 侧相同的诊断短语，方便上层定位问题。
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = ut.must_parse_range("1");
    }));
    assert!(r.is_err());
    let msg = panic_msg(r.unwrap_err());
    assert!(msg.contains("parse range failed"), "{msg}");

    // 结束值小于起始值同样应视为非法范围，而不是静默交换或截断。
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = ut.must_parse_range("5_3");
    }));
    assert!(r.is_err());
    assert!(panic_msg(r.unwrap_err()).contains("parse range failed"));

    // Go 要求范围非负；负数输入必须立即失败。
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = ut.must_parse_range("-1_3");
    }));
    assert!(r.is_err());

    // 非数字整型是所有解析路径的基础失败模式。
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = ut.must_parse_int("x");
    }));
    assert!(r.is_err());

    // 未知命令行参数在 Go `flag` 解析阶段就会终止。
    // Rust 端也需要维持这种“快速失败”的用户体验。
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = stubs::parse_flags(&["-not-a-real-flag".into()]);
    }));
    assert!(r.is_err());

    // SQL 执行失败必须走 Fatal，而不是吞掉错误继续跑后续 job。
    // 这保证压测工具在环境异常时能尽快暴露问题。
    // Execute failure Fatals
    let session = RecordingSession::new();
    session.set_fail_on_sql("boom");
    let mut ut = BenchDB {
        store: ut.store.clone(),
        session,
        flags: Flags::default(),
    };
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        ut.must_exec("boom sql", &[]);
    }));
    assert!(r.is_err());
}

/// Resource cleanup: result set Close runs when execution returns normally.
/// 这一组锁定 `mustExec` 的资源生命周期语义。
/// Go 版本依赖 `defer` 关闭结果集；`log.Fatal` 会直接退出且不会运行 defer，
/// Rust 迁移后同样只在正常返回路径关闭，并把关闭错误作为致命错误处理。
fn contract_resource_cleanup() {
    let base = new_bench_db(&Flags::default(), &SessionFactory::default());
    let session = RecordingSession::new();
    session.set_rows_before_empty(2);
    let mut ut = BenchDB {
        store: base.store,
        session: session.clone(),
        flags: Flags::default(),
    };
    ut.must_exec("select 1", &[]);
    // 即使查询返回多批 chunk，读空结束后也必须准确关闭一次结果集。
    assert_eq!(
        session.close_count(),
        1,
        "mustExec must Close the result set"
    );

    // 关闭失败在 Go 中也是致命错误，不能因为查询主体成功就忽略收尾阶段的问题。
    // Close error is Fatal
    let session2 = RecordingSession::new();
    session2.set_fail_on_close(true);
    let mut ut2 = BenchDB {
        store: ut.store.clone(),
        session: session2,
        flags: Flags::default(),
    };
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        ut2.must_exec("select 2", &[]);
    }));
    assert!(r.is_err());
    assert!(panic_msg(r.unwrap_err()).contains("close failed"));
}

// `catch_unwind` 返回的是擦除类型后的 panic 负载。
// 这个辅助函数把常见字符串负载统一还原成可断言文本，
// 让上面的错误路径测试可以稳定比较关键诊断片段。
fn panic_msg(err: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = err.downcast_ref::<String>() {
        return s.clone();
    }
    if let Some(s) = err.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    format!("{err:?}")
}
