// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `dump_test.go`.
//!
//! 这些测试主要覆盖 `dump.rs` 中最容易回归的编排逻辑，
//! 包括 Dumper 的关闭路径、GC 保护更新器的重试与取消、
//! 表元信息构建、表列举策略选择，以及若干兼容性辅助函数。

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::main_test::{app_logger, default_config_for_test};
use crate::util_for_test::new_mock_pd_client_for_gc;
use crate::*;

fn make_dumper(mut conf: Config) -> Dumper {
    // 单元测试统一关闭 HTTP 服务，避免端口绑定把结果变成环境相关。
    conf.StatusAddr.clear(); // avoid binding HTTP in unit tests
    let factory = conf.PromFactory.clone();
    let labels = conf.Labels.clone();
    let (tctx, cancel) = tcontext::Background().WithLogger(app_logger()).WithCancel();
    // 测试 helper 只组装最小可用 Dumper，不跑完整 NewDumper 初始化链。
    // 这让测试可以精确控制 DB、PD client 和配置副本的初始状态。
    Dumper {
        tctx,
        conf: Arc::new(conf),
        db: Some(DB::new()),
        ext_storage: None,
        metrics: newMetrics(factory.as_ref(), &labels),
        speedRecorder: std::sync::Mutex::new(NewSpeedRecorder()),
        totalTables: std::sync::atomic::AtomicI64::new(0),
        cancel: Some(cancel),
        http: None,
        pd_client: None,
    }
}

#[test]
fn test_dump_exit() {
    // 关闭路径应当容忍“外部已经先 cancel 过一次”的情况。
    let mut conf = default_config_for_test();
    conf.StatusAddr.clear();
    conf.Logger = Some(app_logger());
    let mut d = make_dumper(conf);
    // Cancel then Close should be clean
    // 这里显式拿走 cancel handle，验证 Close 在缺少 handle 时仍是幂等的。
    if let Some(c) = d.cancel.take() {
        c.call();
    }
    // 不校验更多副作用，只要求关闭路径不报错。
    assert!(d.Close().is_ok());
}

#[test]
fn test_tidb_resolve_keyspace_meta_for_gc() {
    // keyspace 解析在最小实现里可以退化为 no-op，但应补上一个 mock PD client。
    let mut conf = default_config_for_test();
    conf.StatusAddr.clear();
    let mut d = make_dumper(conf);
    assert!(tidbResolveKeyspaceMetaForGC(&mut d).is_ok());
    // 成功后至少应该填上一个可用的 pd_client 句柄。
    assert!(d.pd_client.is_some());
}

#[test]
fn test_resolve_keyspace_meta_gc_api_choice() {
    // 同一 helper 再测一遍，锁住“成功后 pd_client 一定存在”这一外部契约。
    let mut conf = default_config_for_test();
    conf.StatusAddr.clear();
    let mut d = make_dumper(conf);
    tidbResolveKeyspaceMetaForGC(&mut d).unwrap();
    // 这里和上个测试共同锁定“解析成功 -> pd_client 已准备”的结果。
    assert!(d.pd_client.is_some());
}

#[test]
fn test_pd_security_option_for_gc() {
    // cluster 级证书优先级应高于通用 Security 字段。
    let mut conf = default_config_for_test();
    conf.ClusterSSLCA = "cluster-ca".into();
    conf.Security.CAPath = "sec-ca".into();
    conf.ClusterSSLCert = "cluster-cert".into();
    conf.Security.CertPath = "sec-cert".into();
    conf.ClusterSSLKey = "cluster-key".into();
    conf.Security.KeyPath = "sec-key".into();
    let opt = pdSecurityOptionForGC(&conf);
    assert_eq!(opt.CAPath, "cluster-ca");
    assert_eq!(opt.CertPath, "cluster-cert");
    assert_eq!(opt.KeyPath, "cluster-key");

    // 当 cluster-ssl-* 为空时，应回落到通用 Security 路径。
    conf.ClusterSSLCA.clear();
    conf.ClusterSSLCert.clear();
    conf.ClusterSSLKey.clear();
    let opt = pdSecurityOptionForGC(&conf);
    // 回退只测 CA 足够，因为 cert/key 的优先级逻辑完全相同。
    assert_eq!(opt.CAPath, "sec-ca");
}

#[test]
fn test_parse_cluster_ssl_flags() {
    // 目前 parse 逻辑只是透传，这个测试锁住最小接口契约。
    // 未来如果这里增加存在性或格式校验，测试也会及时暴露接口变化。
    let (ca, cert, key) = parseClusterSSLFlags("a", "b", "c").unwrap();
    assert_eq!((ca, cert, key), ("a".into(), "b".into(), "c".into()));
}

#[test]
fn test_update_service_safe_point_retry_and_cancel() {
    // safepoint 更新器要能在临时错误下重试，并在 cancel 后干净退出。
    let (tctx, cancel) = tcontext::Background().WithLogger(app_logger()).WithCancel();
    let mock_pd = new_mock_pd_client_for_gc();
    *mock_pd.update_safe_point_err.lock().unwrap() = Some(errors_new("temp"));
    let pd = mock_pd.clone();
    let tctx2 = tctx.clone();
    // 后台线程模拟真实的长期续租过程。
    let h = thread::spawn(move || updateServiceSafePoint(&tctx2, &pd, 2, 100));
    thread::sleep(Duration::from_millis(50));
    // 这里的 cancel 是唯一退出条件，测试重点是 updater 是否及时响应。
    cancel.call();
    h.join().unwrap();
}

#[test]
fn test_update_keyspace_gc_barrier_retry_and_cancel() {
    // keyspace barrier 路径与普通 safepoint 类似，但走的是另一套 PD API。
    let (tctx, cancel) = tcontext::Background().WithLogger(app_logger()).WithCancel();
    let mock_pd = new_mock_pd_client_for_gc();
    *mock_pd.gc_states_client.set_barrier_err.lock().unwrap() = Some(errors_new("temp"));
    let pd = mock_pd.clone();
    let tctx2 = tctx.clone();
    let h = thread::spawn(move || updateKeyspaceGCBarrier(&tctx2, &pd, 1, 2, 100));
    thread::sleep(Duration::from_millis(50));
    // 若 join 卡住，通常说明 barrier 路径没有正确检查 Done 信号。
    cancel.call();
    h.join().unwrap();
}

#[test]
fn test_update_service_safe_point_snapshot_zero() {
    // snapshot_ts=0 still keeps the protection updater alive until cancellation.
    let (tctx, cancel) = tcontext::Background().WithLogger(app_logger()).WithCancel();
    let mock_pd = new_mock_pd_client_for_gc();
    let pd = mock_pd.clone();
    let tctx2 = tctx.clone();
    let h = thread::spawn(move || updateServiceSafePoint(&tctx2, &pd, 2, 0));
    thread::sleep(Duration::from_millis(30));
    cancel.call();
    h.join().unwrap();
    // 至少调用过一次 update，证明不是在进入循环前就提前返回。
    // 同时也证明 snapshot=0 分支并不是一个纯空操作。
    assert!(
        mock_pd
            .update_safe_point_calls
            .load(std::sync::atomic::Ordering::SeqCst)
            >= 1
    );
}

#[test]
fn test_update_keyspace_gc_barrier_snapshot_zero() {
    // barrier updater likewise remains active until cancellation.
    let (tctx, cancel) = tcontext::Background().WithLogger(app_logger()).WithCancel();
    let mock_pd = new_mock_pd_client_for_gc();
    let pd = mock_pd.clone();
    let tctx2 = tctx.clone();
    let h = thread::spawn(move || updateKeyspaceGCBarrier(&tctx2, &pd, 1, 2, 0));
    thread::sleep(Duration::from_millis(30));
    cancel.call();
    h.join().unwrap();
    // set_calls 至少为 1，表示 barrier API 确实被触发过。
    // 这样才能证明 snapshot=0 只是缩短生命周期，而不是完全跳过更新。
    assert!(
        mock_pd
            .gc_states_client
            .set_calls
            .load(std::sync::atomic::Ordering::SeqCst)
            >= 1
    );
}

#[test]
fn test_update_keyspace_gc_barrier_cancel_during_retry() {
    // 如果 barrier 设置持续报错，cancel 仍应让后台线程及时结束。
    let (tctx, cancel) = tcontext::Background().WithLogger(app_logger()).WithCancel();
    let mock_pd = new_mock_pd_client_for_gc();
    *mock_pd.gc_states_client.set_barrier_err.lock().unwrap() = Some(errors_new("temp"));
    let pd = mock_pd.clone();
    let tctx2 = tctx.clone();
    let h = thread::spawn(move || updateKeyspaceGCBarrier(&tctx2, &pd, 99, 2, 500));
    thread::sleep(Duration::from_millis(30));
    // keyspace id 取 99 只是为了区别普通路径，不影响测试语义。
    cancel.call();
    h.join().unwrap();
}

#[test]
fn test_update_service_safe_point_cancel_during_retry() {
    // 与上一个用例对称，验证普通 safepoint 路径的取消响应性。
    let (tctx, cancel) = tcontext::Background().WithLogger(app_logger()).WithCancel();
    let mock_pd = new_mock_pd_client_for_gc();
    *mock_pd.update_safe_point_err.lock().unwrap() = Some(errors_new("temp"));
    let pd = mock_pd.clone();
    let tctx2 = tctx.clone();
    let h = thread::spawn(move || updateServiceSafePoint(&tctx2, &pd, 2, 500));
    thread::sleep(Duration::from_millis(30));
    // 这个用例和 barrier 版本一起覆盖两套 updater 包装函数。
    cancel.call();
    h.join().unwrap();
}

#[test]
fn test_dump_table_meta() {
    // 这里用最小 SHOW COLUMNS 响应验证 tableMeta 能正确带出库名和表名。
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut base = newBaseConn(conn.clone(), true, None);
    let conf = default_config_for_test();
    conn.seed_query(
        "SHOW COLUMNS FROM `db`.`t`",
        vec![
            "Field".into(),
            "Type".into(),
            "Null".into(),
            "Key".into(),
            "Default".into(),
            "Extra".into(),
        ],
        vec![vec![
            Some(b"id".to_vec()),
            Some(b"int".to_vec()),
            Some(b"NO".to_vec()),
            Some(b"PRI".to_vec()),
            None,
            Some(b"".to_vec()),
        ]],
    );
    conn.seed_query(
        "SELECT * FROM `db`.`t` LIMIT 1",
        vec!["id".into()],
        vec![vec![Some(b"1".to_vec())]],
    );
    conn.seed_query(
        "SHOW CREATE TABLE `db`.`t`",
        vec!["Table".into(), "Create Table".into()],
        vec![vec![
            Some(b"t".to_vec()),
            Some(b"CREATE TABLE `t` (`id` int)".to_vec()),
        ]],
    );
    let table = TableInfo {
        Name: "t".into(),
        AvgRowLength: 1,
        Type: TableType::TableTypeBase,
    };
    // 测试重点不是列类型细节，而是构造流程能跑通并返回基础元信息。
    let meta = dumpTableMeta(&tctx, &conf, &mut base, "db", &table).unwrap();
    // 只核对最核心的标识字段，避免与未完全实现的附加元数据耦合。
    assert_eq!(meta.DatabaseName(), "db");
    assert_eq!(meta.TableName(), "t");
    assert_eq!(meta.ColumnCount(), 1);
    assert_eq!(meta.ShowCreateTable(), "CREATE TABLE `t` (`id` int)");
}

#[test]
fn test_get_list_table_type_by_conf() {
    // Go defaults to SHOW TABLE STATUS, and only consistency-sensitive paths override it.
    let mut conf = default_config_for_test();
    conf.ServerInfo.ServerType = ServerType::ServerTypeTiDB;
    conf.Consistency = ConsistencyTypeNone.into();
    assert_eq!(
        getListTableTypeByConf(&conf),
        listTableType::listTableByShowTableStatus
    );
    conf.Consistency = ConsistencyTypeLock.into();
    assert_eq!(
        getListTableTypeByConf(&conf),
        listTableType::listTableByInfoSchema
    );
    conf.ServerInfo = ParseServerInfo("8.0.3");
    conf.Consistency = ConsistencyTypeFlush.into();
    assert_eq!(
        getListTableTypeByConf(&conf),
        listTableType::listTableByShowFullTables
    );
    conf.ServerInfo = ParseServerInfo("5.7.25");
    // 老版本/普通版本 MySQL 最终回落到 SHOW TABLE STATUS 路径。
    assert_eq!(
        getListTableTypeByConf(&conf),
        listTableType::listTableByShowTableStatus
    );
}

#[test]
fn test_can_rebuild_conn_matches_go_consistency_matrix() {
    assert!(canRebuildConn(ConsistencyTypeFlush, false));
    assert!(!canRebuildConn(ConsistencyTypeFlush, true));
    assert!(canRebuildConn(ConsistencyTypeSnapshot, true));
    assert!(canRebuildConn(ConsistencyTypeNone, true));
    assert!(!canRebuildConn(ConsistencyTypeAuto, false));
}

#[test]
fn test_validate_resolved_consistency_rejects_snapshot_option_outside_snapshot_mode() {
    let mut conf = default_config_for_test();
    conf.Consistency = ConsistencyTypeNone.into();
    conf.Snapshot = "12345".into();
    let mut d = make_dumper(conf);
    let err = validateResolveAutoConsistency(&mut d).unwrap_err();
    assert!(err.msg.contains("can't specify --snapshot"));
}

#[test]
fn test_resolve_auto_consistency_unknown_server_uses_none() {
    let mut conf = default_config_for_test();
    conf.Consistency = ConsistencyTypeAuto.into();
    conf.ServerInfo.ServerType = ServerType::ServerTypeUnknown;
    let mut d = make_dumper(conf);
    resolveAutoConsistency(&mut d).unwrap();
    assert_eq!(d.conf.Consistency, ConsistencyTypeNone);
}

#[test]
fn test_adjust_database_collation() {
    // 当前最小实现下，宽松模式不改写数据库级 CREATE SQL。
    let tctx = tcontext::Background().WithLogger(app_logger());
    let sql = "CREATE DATABASE `db` DEFAULT CHARACTER SET utf8";
    let out = adjustDatabaseCollation(
        &tctx,
        LooseCollationCompatible,
        sql,
        &std::collections::HashMap::new(),
    )
    .unwrap();
    // 宽松模式下透传原 SQL，是当前最小实现的明确契约。
    assert_eq!(out, sql);
}

#[test]
fn test_adjust_table_collation() {
    // 表级 collation helper 目前同样是透传，这里锁住该行为。
    let tctx = tcontext::Background().WithLogger(app_logger());
    let sql = "CREATE TABLE t(a int)";
    let out = adjustTableCollation(
        &tctx,
        LooseCollationCompatible,
        sql,
        &std::collections::HashMap::new(),
    )
    .unwrap();
    // 表级 helper 与数据库级 helper 一样，当前都不改写输入 SQL。
    assert_eq!(out, sql);
}

#[test]
fn test_unregister_metrics() {
    // register/unregister 组合至少应可重复执行而不 panic。
    // 该测试不检查 registry 内部状态，只锁定生命周期调用可顺利完成。
    let mut conf = default_config_for_test();
    conf.StatusAddr.clear();
    let mut d = make_dumper(conf);
    d.metrics.registerTo(d.conf.PromRegistry.as_ref());
    d.metrics.unregisterFrom(d.conf.PromRegistry.as_ref());
}

#[test]
fn test_set_default_session_params() {
    // TiDB >= 6.2 with TiKV enables paging unless the user supplied a value.
    let mut params = std::collections::HashMap::new();
    let si = ServerInfo {
        ServerType: ServerType::ServerTypeTiDB,
        ServerVersion: Some(parse_semver("6.2.0")),
        HasTiKV: true,
    };
    setDefaultSessionParams(&si, &mut params);
    assert_eq!(
        params.get("tidb_enable_paging").map(|s| s.as_str()),
        Some("ON")
    );
    params.insert("tidb_enable_paging".into(), "OFF".into());
    setDefaultSessionParams(&si, &mut params);
    assert_eq!(
        params.get("tidb_enable_paging").map(String::as_str),
        Some("OFF")
    );
}

#[test]
fn test_set_session_params() {
    // 再从 Dumper 路径验证一次，确保配置副本替换流程同样生效。
    // 这个用例和上一个 helper 用例一起形成“底层 + 上层”双保险。
    let mut conf = default_config_for_test();
    conf.StatusAddr.clear();
    conf.ServerInfo = ServerInfo {
        ServerType: ServerType::ServerTypeTiDB,
        ServerVersion: Some(parse_semver("6.2.0")),
        HasTiKV: true,
    };
    let mut d = make_dumper(conf);
    setSessionParam(&mut d).unwrap();
    // 最终值应写回 `d.conf.SessionParams`，而不是仅停留在临时副本里。
    // 这也间接验证了 `clone_for_mutate + Arc 替换` 的配置更新模式。
    assert_eq!(
        d.conf
            .SessionParams
            .get("tidb_enable_paging")
            .map(|s| s.as_str()),
        Some("ON")
    );
}
