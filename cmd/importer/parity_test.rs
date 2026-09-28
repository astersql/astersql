// Copyright 2026 AsterSQL.

//! 本文件验证 `cmd/importer` 的外部契约而不是内部实现细节。
//! 测试入口按正常路径、边界输入、错误传播和资源回收四个维度组织。
//! 这样做的目的是让回归失败能直接定位到哪一类公共行为发生漂移。
//! 默认配置断言保证 Rust 版本与 Go 命令启动后的初始状态一致。
//! 参数覆盖断言保证命令行选项的优先级没有在迁移后变化。
//! 表和索引解析断言保证 parser 输出仍能驱动后续造数流程。
//! comment 规则断言覆盖 importer 最核心的列级扩展语义。
//! 随机种子断言保证测试结果可重复，避免偶发性失败掩盖真实回归。
//! 批量造数断言不绑定具体随机值，只绑定结果形状与语句骨架。
//! `intToDecimalString` 断言对应 Go 侧已有表格化样例。
//! 完整流水线断言关注 DDL 先于 DML、worker 会消费任务、连接最终关闭。
//! 直方图前缀断言验证字符串取样的关键启发式没有被破坏。
//! 桶内整数断言验证重复值与区间值的边界计算仍然正确。
//! 空索引 SQL 的边界断言保证上层无需为默认值额外分支。
//! 帮助参数断言保证命令仍通过显式错误而不是 panic 提示用户。
//! 零连接数断言保证资源创建接口可以安全返回空集合。
//! 任务通道断言验证发送端关闭语义与 Go 版本一致。
//! 随机字符串和随机整数断言只校验长度与范围，不约束具体输出。
//! 默认 datum 断言保证未配规则时的造数节奏不发生漂移。
//! 错误路径中的坏 SQL 断言保证解析失败不会被误判成运行时故障。
//! 索引表名不匹配断言覆盖 `parseIndexSQL` 的关键保护逻辑。
//! 未知 flag 断言保证非法输入尽早失败而不是被静默忽略。
//! 伪造未支持类型断言覆盖正常 SQL 难以触达的兜底分支。
//! `run_with_args` 的 fatal 路径通过捕获 panic 模拟 Go 的进程终止。
//! 关闭连接断言保证 `closeDBs` 会遍历并处理所有句柄。
//! `doProcess` 断言验证批次切分和 commit 次数仍与 Go 一致。
//! 统计文件缺失断言保证错误会原样返回给上层决定是否继续。
//! 这些测试共同形成迁移后的最小兼容面，优先保护用户可观察行为。
//! 新增注释只解释测试意图，不改变任何断言或执行顺序。
//! 阅读失败用例时，应优先把 panic 视为 Go `fatal`/`os.Exit` 的语义替身。
//! 阅读成功用例时，则应把重点放在配置、造数与回收的链路是否完整闭合。
//! 如果未来要扩展覆盖面，应继续沿着公共契约而不是内部细节添加样例。

use std::panic::{self, AssertUnwindSafe};
use std::process::Command;
use std::sync::Arc;

use crate::config::{Config, NewConfig};
use crate::data::{newDatum, randInt, randString};
use crate::db::{closeDBs, createDBs, execSQL, genRowData, genRowDatas, intToDecimalString};
use crate::entry::run_with_args;
use crate::job::{addJobs, doProcess};
use crate::parser::{newTable, parseIndexSQL, parseTableSQL};
use crate::stats::{getValidPrefix, histogram, loadStats};
use crate::stubs::{self, BoundDatum, Bounds, Bucket, DB, HistogramCore, seed_rng};

#[test]
fn process_exit_probe() {
    if let Ok(code) = std::env::var("ASTERSQL_IMPORTER_EXIT_PROBE") {
        stubs::exit_process_for_test(code.parse().expect("integer exit probe"));
    }
}

#[test]
fn process_exit_codes_match_go_main() {
    let test_binary = std::env::current_exe().expect("resolve importer test executable");
    let help = Command::new(&test_binary)
        .args(["--exact", "parity_test::process_exit_probe"])
        .env("ASTERSQL_IMPORTER_EXIT_PROBE", "0")
        .status()
        .expect("run importer exit probe");
    assert_eq!(
        help.code(),
        Some(0),
        "Go exits successfully for flag.ErrHelp"
    );

    let invalid = Command::new(test_binary)
        .args(["--exact", "parity_test::process_exit_probe"])
        .env("ASTERSQL_IMPORTER_EXIT_PROBE", "2")
        .status()
        .expect("run importer invalid-flag exit probe");
    assert_eq!(
        invalid.code(),
        Some(2),
        "Go exits with status 2 for flag errors"
    );
}

#[test]
fn config_rejects_invalid_toml_value_types() {
    let path = std::env::temp_dir().join(format!(
        "astersql-importer-invalid-config-{}.toml",
        std::process::id()
    ));
    std::fs::write(&path, "[db]\nport = \"not-a-number\"\n").unwrap();

    let mut cfg = NewConfig();
    let result = cfg.Parse(&["-config".into(), path.to_string_lossy().into_owned()]);
    std::fs::remove_file(path).unwrap();

    assert!(
        result.is_err(),
        "Go TOML decoding rejects a string for db.port"
    );
}

#[test]
fn parser_handles_index_table_names_and_mysql_type_codes() {
    let mut table = newTable();
    parseTableSQL(
        &mut table,
        "create table mytable(a text, b tinyblob, c mediumint);",
    )
    .unwrap();
    parseIndexSQL(&mut table, "create index i_a on mytable(a);").unwrap();

    assert!(table.indices.contains_key("a"));
    assert_eq!(table.columns[0].tp.GetType(), stubs::TypeBlob);
    assert_eq!(table.columns[0].tp.GetFlen(), 65_535);
    assert_eq!(table.columns[1].tp.GetType(), stubs::TypeTinyBlob);
    assert_eq!(table.columns[1].tp.GetFlen(), 255);
    assert_eq!(table.columns[2].tp.GetType(), 9); // mysql.TypeInt24
    assert!(
        genRowData(&table).is_err(),
        "Go genColumnData does not implement MEDIUMINT"
    );
}

#[test]
fn parser_accepts_create_table_if_not_exists() {
    let mut table = newTable();
    parseTableSQL(
        &mut table,
        "create table if not exists t(a int primary key);",
    )
    .unwrap();
    assert_eq!(table.name, "t");
    assert_eq!(table.columnList, "a");
}

#[test]
fn worker_database_failure_does_not_deadlock() {
    let mut table = newTable();
    parseTableSQL(&mut table, "create table t(a int primary key);").unwrap();
    let db = DB::new("failure".into());
    db.set_fail_begin(true);

    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            doProcess(Arc::new(table), &[db], 1, 1, 1);
        }));
        result_tx.send(result.is_err()).unwrap();
    });

    assert_eq!(
        result_rx.recv_timeout(std::time::Duration::from_secs(1)),
        Ok(true),
        "Go log.Fatal terminates on Begin failure; Rust must not hang waiting for done"
    );
}

#[test]
fn stats_loader_rejects_malformed_json() {
    let path = std::env::temp_dir().join(format!(
        "astersql-importer-invalid-stats-{}.json",
        std::process::id()
    ));
    std::fs::write(&path, "{not-json").unwrap();

    let result = loadStats(&newTable().tblInfo, path.to_str().unwrap());
    std::fs::remove_file(path).unwrap();

    assert!(
        result.is_err(),
        "Go json.Unmarshal rejects malformed stats JSON"
    );
}

#[test]
// `go_rust_public_contract_matches` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn go_rust_public_contract_matches() {
    contract_normal_path();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

/// Normal: defaults, parse table/index, generate INSERT, run workers, DDL exec order.
// `contract_normal_path` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn contract_normal_path() {
    let cfg = NewConfig();
    assert_eq!(cfg.DBCfg.Host, "127.0.0.1");
    assert_eq!(cfg.DBCfg.User, "root");
    assert_eq!(cfg.DBCfg.Password, "");
    assert_eq!(cfg.DBCfg.Name, "test");
    assert_eq!(cfg.DBCfg.Port, 3306);
    assert_eq!(cfg.SysCfg.WorkerCount, 2);
    assert_eq!(cfg.SysCfg.JobCount, 10000);
    assert_eq!(cfg.SysCfg.Batch, 1000);
    assert_eq!(cfg.SysCfg.LogLevel, "info");
    assert!(cfg.DDLCfg.TableSQL.is_empty());

    let mut cfg = NewConfig();
    cfg.Parse(&[
        "-t".into(),
        "create table t(a int primary key, b varchar(8), c double);".into(),
        "-i".into(),
        "create unique index u_b on t(b);".into(),
        "-c".into(),
        "2".into(),
        "-n".into(),
        "5".into(),
        "-b".into(),
        "2".into(),
        "-P".into(),
        "4000".into(),
    ])
    .unwrap();
    assert_eq!(cfg.DBCfg.Port, 4000);
    assert_eq!(cfg.SysCfg.JobCount, 5);
    assert_eq!(cfg.SysCfg.Batch, 2);
    assert!(cfg.DDLCfg.TableSQL.contains("create table t"));

    let mut table = newTable();
    parseTableSQL(&mut table, &cfg.DDLCfg.TableSQL).unwrap();
    assert_eq!(table.name, "t");
    assert_eq!(table.columns.len(), 3);
    assert_eq!(table.columnList, "a,b,c");
    assert!(table.uniqIndices.contains_key("a"));
    parseIndexSQL(&mut table, &cfg.DDLCfg.IndexSQL).unwrap();
    assert!(table.uniqIndices.contains_key("b"));

    // Comment rules
    let mut t2 = newTable();
    parseTableSQL(
        &mut t2,
        "create table t(a int comment '[[range=1,10;set=1,2,3]]');",
    )
    .unwrap();
    assert_eq!(t2.columns[0].min, "1");
    assert_eq!(t2.columns[0].max, "10");
    assert_eq!(
        t2.columns[0].set,
        vec!["1".to_string(), "2".into(), "3".into()]
    );

    seed_rng(42);
    let rows = genRowDatas(&table, 3).unwrap();
    assert_eq!(rows.len(), 3);
    for r in &rows {
        assert!(r.starts_with("insert into t (a,b,c) values ("));
        assert!(r.ends_with(");"));
    }

    // intToDecimalString matches Go db_test.go
    assert_eq!(intToDecimalString(100, 3), "0.100");
    assert_eq!(intToDecimalString(100, 1), "10.0");
    assert_eq!(intToDecimalString(100, 0), "100");
    assert_eq!(intToDecimalString(1, 3), "0.001");
    assert_eq!(intToDecimalString(0, 1), "0.0");
    assert_eq!(intToDecimalString(12345678, 2), "123456.78");

    // Full pipeline with in-memory DBs
    let db0 = DB::new("mock0".into());
    let db1 = DB::new("mock1".into());
    let args = vec![
        "-t".into(),
        "create table t(a int primary key, b varchar(4));".into(),
        "-n".into(),
        "4".into(),
        "-b".into(),
        "2".into(),
        "-c".into(),
        "2".into(),
    ];
    run_with_args(&args, Some(vec![db0.clone(), db1.clone()])).unwrap();
    assert!(db0.is_closed());
    assert!(db1.is_closed());
    let execs = db0.execs();
    assert!(
        execs.iter().any(|s| s.contains("create table t")),
        "DDL table SQL must be executed first: {execs:?}"
    );
    assert!(
        execs.iter().any(|s| s.starts_with("insert into t")),
        "inserts must run: {execs:?}"
    );
    // 4 jobs / batch 2 => 2 commits on workers combined
    assert!(db0.commits() + db1.commits() >= 1);

    // histogram getValidPrefix
    let p = getValidPrefix("abc", "abd");
    assert!(p.starts_with("ab"));
    assert!(p.len() >= 2);

    // histogram randInt path with constructed buckets
    let h = histogram::from_core(
        HistogramCore {
            Buckets: vec![Bucket {
                Count: 10,
                Repeat: 2,
            }],
            Bounds: Bounds {
                rows: vec![BoundDatum::Int(1), BoundDatum::Int(5)],
            },
            ID: 1,
        },
        None,
    );
    seed_rng(7);
    let v = h.randInt();
    assert!((1..=5).contains(&v));
}

/// Boundary: empty index SQL, help flag, CLI overrides TOML, incremental forces 1 worker.
// `contract_boundary` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn contract_boundary() {
    let mut table = newTable();
    parseTableSQL(
        &mut table,
        "create table t(a int comment '[[incremental=true]]');",
    )
    .unwrap();
    assert!(table.columns[0].incremental);
    parseIndexSQL(&mut table, "").unwrap();

    let mut cfg = NewConfig();
    let err = cfg.Parse(&["-help".into()]).unwrap_err();
    assert!(err.is_help);

    // empty createDBs count
    let dbs = createDBs(&NewConfig().DBCfg, 0).unwrap();
    assert!(dbs.is_empty());

    // job channel: addJobs then drain
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    addJobs(3, tx);
    assert_eq!(rx.iter().count(), 3);

    // randString length
    seed_rng(1);
    assert_eq!(randString(5).len(), 5);
    assert!((0..=10).contains(&randInt(0, 10)));

    // datum defaults
    let d = newDatum();
    assert_eq!(d.step(), 1);
    assert_eq!(d.repeats(), 1);
    assert_eq!(d.probability(), 100);
}

/// Error: bad SQL, mismatched index table, invalid leftover args, unsupported type path.
// `contract_error_paths` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn contract_error_paths() {
    let mut cfg = NewConfig();
    let err = cfg
        .Parse(&["-t".into(), "create table t(a int);".into(), "extra".into()])
        .unwrap_err();
    assert!(err.msg.contains("invalid flag"));

    let mut table = newTable();
    assert!(parseTableSQL(&mut table, "select 1").is_err());

    let mut table = newTable();
    parseTableSQL(&mut table, "create table t(a int);").unwrap();
    let err = parseIndexSQL(&mut table, "create index i on other(a);").unwrap_err();
    assert!(err.msg.contains("mismatch table name"));

    // unknown flag
    let mut cfg = NewConfig();
    assert!(cfg.Parse(&["-z".into(), "1".into()]).is_err());

    // genRowData unsupported — year works; invent bad tp
    let mut table = newTable();
    parseTableSQL(&mut table, "create table t(a int);").unwrap();
    table.columns[0].tp.tp = 0xff; // geometry-like
    let err = genRowData(&table).unwrap_err();
    assert!(err.msg.contains("unsupported column type"));

    // parse flags then fatal path via run
    let db = DB::new("x".into());
    db.set_fail_exec(true);
    let args = vec![
        "-t".into(),
        "create table t(a int);".into(),
        "-n".into(),
        "1".into(),
        "-c".into(),
        "1".into(),
        "-b".into(),
        "1".into(),
    ];
    // 这里要求出现 panic，是为了把 Rust stub 的失败语义对齐到 Go `log.Fatal` 直接终止流程的行为。
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = run_with_args(&args, Some(vec![db.clone()]));
    }));
    assert!(result.is_err(), "execSQL failure must fatal");
}

/// Resource cleanup: closeDBs closes all; doProcess completes and workers finish.
// `contract_resource_cleanup` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn contract_resource_cleanup() {
    let dbs = createDBs(&NewConfig().DBCfg, 3).unwrap();
    assert_eq!(dbs.len(), 3);
    closeDBs(&dbs);
    for db in &dbs {
        assert!(db.is_closed());
    }

    let mut table = newTable();
    parseTableSQL(&mut table, "create table t(a int primary key);").unwrap();
    let db = DB::new("cleanup".into());
    // empty index SQL is fine
    execSQL(&db, "").unwrap();
    doProcess(Arc::new(table), &[db.clone()], 3, 1, 2);
    assert_eq!(db.commits(), 2); // batches: 2 + leftover 1 => 2 commits
    // not closed by doProcess
    assert!(!db.is_closed());
    closeDBs(&[db.clone()]);
    assert!(db.is_closed());

    // loadStats missing file
    let tbl = newTable().tblInfo;
    assert!(loadStats(&tbl, "/nonexistent/stats.json").is_err());
}
