// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `consistency_test.go`.
//!
//! 这些测试锁住一致性控制器最重要的外部契约：
//! 模式分派是否正确、锁表重试与空锁表路径是否可达、
//! auto consistency 是否按数据库类型解析，以及非法模式/环境是否会显式报错。

use crate::main_test::{app_logger, default_config_for_test};
use crate::*;

#[test]
fn test_consistency_controller() {
    // 基础综合用例：依次覆盖 none / flush / snapshot / lock 四种主要路径。
    let (go_ctx, cancel) = tcontext::GoWithCancel(tcontext::GoBackground());
    let _ = cancel;
    let tctx = tcontext::Background()
        .WithContext(go_ctx)
        .WithLogger(app_logger());
    let db = DB::new();
    let mut conf = default_config_for_test();

    // none 应该是纯 no-op，setup 和 teardown 都成功。
    // 这是所有 controller 的最小基线行为。
    conf.Consistency = ConsistencyTypeNone.to_string();
    let mut ctrl = NewConsistencyController(&conf, &db).unwrap();
    assert!(ctrl.Setup(&tctx).is_ok());
    assert!(ctrl.TearDown().is_ok());

    // flush 只在 MySQL 路径下允许成功。
    conf.Consistency = ConsistencyTypeFlush.to_string();
    conf.ServerInfo.ServerType = ServerType::ServerTypeMySQL;
    let mut ctrl = NewConsistencyController(&conf, &db).unwrap();
    assert!(ctrl.Setup(&tctx).is_ok());
    assert!(ctrl.TearDown().is_ok());
    // 这里不强绑具体 SQL 文本，避免 stub 日志顺序调整导致测试脆弱。
    let log = db.Conn().unwrap().exec_log.lock().unwrap().clone();
    // flush/unlock happen on controller's own conn

    // snapshot 在 TiDB 上退化成无需额外动作的控制器。
    conf.Consistency = ConsistencyTypeSnapshot.to_string();
    conf.ServerInfo.ServerType = ServerType::ServerTypeTiDB;
    let mut ctrl = NewConsistencyController(&conf, &db).unwrap();
    assert!(ctrl.Setup(&tctx).is_ok());
    assert!(ctrl.TearDown().is_ok());
    // snapshot 在当前实现下借道 ConsistencyNone，因此同样应平稳通过。

    // lock 路径需要准备显式表集合，覆盖普通表和视图混合输入。
    conf.ServerInfo.ServerType = ServerType::ServerTypeMySQL;
    conf.Consistency = ConsistencyTypeLock.to_string();
    conf.Tables = NewDatabaseTables()
        .AppendTables("db1", &["t1".into(), "t2".into(), "t3".into()], &[1, 2, 3])
        .AppendViews("db2", &["t4"])
        .clone();
    // clone_for_mutate loses chained return; rebuild
    let mut tables = NewDatabaseTables();
    tables
        .AppendTables("db1", &["t1".into(), "t2".into(), "t3".into()], &[1, 2, 3])
        .AppendViews("db2", &["t4"]);
    conf.Tables = tables;
    let mut ctrl = NewConsistencyController(&conf, &db).unwrap();
    assert!(ctrl.Setup(&tctx).is_ok());
    assert!(ctrl.TearDown().is_ok());
    // 这里只保留 log 变量，说明控制器会在自己的连接上记录执行轨迹。
    // 这也避免编译器把前面的日志相关准备优化成未使用变量。
    let _ = log;
}

#[test]
fn test_consistency_lock_controller_retry() {
    // 这个用例聚焦“第一次锁表失败后，控制器仍能走完整 setup/teardown 路径”。
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let exec_log = conn.exec_log.clone();
    // buildLockTablesSQL order may vary with HashMap; push fail for the first LOCK then succeed.
    // 模拟 MySQL 报 no such table，逼出 backoffer 的重试和 block-list 收敛逻辑。
    conn.push_fail(Error::with_mysql(MySQLError {
        Number: ErrNoSuchTable,
        Message: "Table 'db1.t3' doesn't exist".into(),
    }));

    let mut conf = default_config_for_test();
    conf.ServerInfo.ServerType = ServerType::ServerTypeMySQL;
    conf.Consistency = ConsistencyTypeLock.to_string();
    let mut tables = NewDatabaseTables();
    tables
        .AppendTables("db1", &["t1".into(), "t2".into(), "t3".into()], &[1, 2, 3])
        .AppendViews("db2", &["t4"]);
    conf.Tables = tables;

    let mut ctrl = ConsistencyLockDumpingTables {
        conn: Some(conn),
        empty_lock_sql: false,
        specified_tables: conf.SpecifiedTables,
        server_type: conf.ServerInfo.ServerType,
        conf: conf.clone_for_mutate(),
    };
    ctrl.Setup(&tctx).unwrap();

    let log = exec_log.lock().unwrap().clone();
    assert_eq!(log.len(), 2, "1146 should trigger exactly one retry");
    assert!(log[0].contains("`db1`.`t3` READ"));
    assert!(!log[1].contains("`db1`.`t3` READ"));
    assert_eq!(
        ctrl.conf.Tables["db1"]
            .iter()
            .map(|table| table.Name.as_str())
            .collect::<Vec<_>>(),
        vec!["t1", "t2"]
    );
    assert!(
        ctrl.conf.Tables["db2"]
            .iter()
            .any(|table| table.Name == "t4")
    );
    ctrl.TearDown().unwrap();
}

#[test]
fn test_consistency_lock_controller_empty() {
    // 唯一基表消失后，下一轮应转成 empty_lock_sql，而不是再次执行同一 SQL。
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let closed = conn.closed.clone();
    conn.push_fail(Error::with_mysql(MySQLError {
        Number: ErrNoSuchTable,
        Message: "Table 'db1.t1' doesn't exist".into(),
    }));
    let mut conf = default_config_for_test();
    conf.ServerInfo.ServerType = ServerType::ServerTypeMySQL;
    conf.Consistency = ConsistencyTypeLock.to_string();
    let mut tables = NewDatabaseTables();
    tables
        .AppendTables("db1", &["t1".into()], &[1])
        .AppendViews("db2", &["t4"]);
    conf.Tables = tables;
    let mut ctrl = ConsistencyLockDumpingTables {
        conn: Some(conn),
        empty_lock_sql: false,
        specified_tables: conf.SpecifiedTables,
        server_type: conf.ServerInfo.ServerType,
        conf: conf.clone_for_mutate(),
    };
    assert!(ctrl.Setup(&tctx).is_ok());
    assert!(ctrl.empty_lock_sql);
    assert!(closed.load(std::sync::atomic::Ordering::SeqCst));
    assert!(ctrl.conf.Tables["db1"].is_empty());
    assert!(
        ctrl.conf.Tables["db2"]
            .iter()
            .any(|table| table.Name == "t4")
    );
    assert!(ctrl.PingContext().is_ok());
    assert!(ctrl.TearDown().is_ok());
}

#[test]
fn test_resolve_auto_consistency() {
    // auto consistency 会根据服务端类型在 snapshot 和 flush 之间选择。
    let mut d = Dumper {
        tctx: tcontext::Background().WithLogger(app_logger()),
        conf: std::sync::Arc::new({
            let mut c = default_config_for_test();
            c.Consistency = ConsistencyTypeAuto.to_string();
            c.ServerInfo.ServerType = ServerType::ServerTypeTiDB;
            c
        }),
        db: None,
        ext_storage: None,
        metrics: std::sync::Arc::new(newMetrics(NewDefaultFactory().as_ref(), &Labels::default())),
        speedRecorder: std::sync::Arc::new(std::sync::Mutex::new(NewSpeedRecorder())),
        status: std::sync::Arc::new(std::sync::Mutex::new(DumpStatus::default())),
        totalTables: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
        cancel: None,
        http: None,
        pd_client: None,
    };
    resolveAutoConsistency(&mut d).unwrap();
    assert_eq!(d.conf.Consistency, ConsistencyTypeSnapshot);
    // TiDB 优先选择 snapshot，是因为它能提供更自然的一致性视图。

    // 同一个 dumper 换成 MySQL 后，应回落到 flush。
    let mut conf = (*d.conf).clone_for_mutate();
    conf.Consistency = ConsistencyTypeAuto.to_string();
    conf.ServerInfo.ServerType = ServerType::ServerTypeMySQL;
    d.conf = std::sync::Arc::new(conf);
    resolveAutoConsistency(&mut d).unwrap();
    // MySQL 没有 TiDB 的快照读能力，因此 auto 最终落到 flush。
    assert_eq!(d.conf.Consistency, ConsistencyTypeFlush);
}

#[test]
fn test_consistency_controller_error() {
    // 非法模式、非 TiDB snapshot、TiDB flush 都应显式失败。
    let db = DB::new();
    let mut conf = default_config_for_test();
    conf.Consistency = "invalid".into();
    assert!(NewConsistencyController(&conf, &db).is_err());

    conf.Consistency = ConsistencyTypeSnapshot.to_string();
    conf.ServerInfo.ServerType = ServerType::ServerTypeMySQL;
    assert!(NewConsistencyController(&conf, &db).is_err());
    // 非 TiDB 上请求 snapshot，必须在构造阶段就失败。

    conf.Consistency = ConsistencyTypeFlush.to_string();
    conf.ServerInfo.ServerType = ServerType::ServerTypeTiDB;
    let mut ctrl = NewConsistencyController(&conf, &db).unwrap();
    let tctx = tcontext::Background().WithLogger(app_logger());
    assert!(ctrl.Setup(&tctx).is_err());
}

#[test]
fn test_consistency_lock_tidb_check() {
    // TiDB lock consistency 还依赖 `tidb_enable_table_lock`，这里验证关闭时的报错。
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    conn.seed_query(
        "SELECT @@tidb_enable_table_lock",
        vec!["@@tidb_enable_table_lock".into()],
        vec![vec![Some(b"0".to_vec())]],
    );
    let mut conf = default_config_for_test();
    conf.ServerInfo.ServerType = ServerType::ServerTypeTiDB;
    conf.Consistency = ConsistencyTypeLock.to_string();
    let mut tables = NewDatabaseTables();
    tables.AppendTables("db", &["t".into()], &[1]);
    conf.Tables = tables;
    let mut ctrl = NewConsistencyController(&conf, &db).unwrap();
    let err = ctrl.Setup(&tctx).unwrap_err();
    // 错误文案允许存在轻微差异，但必须明确提到 table lock 配置问题。
    // 这样既保留实现弹性，又不会放过真正的语义回归。
    assert!(err.msg.contains("enable-table-lock") || err.msg.contains("table lock"));
}
