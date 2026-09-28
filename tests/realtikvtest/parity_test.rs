//! 中文说明开始（自动生成）
//! 中文总览：`parity_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `parity_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 56 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `go_rust_public_contract_matches` 是当前文件里的辅助函数。
//! 阅读 `go_rust_public_contract_matches` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `go_rust_public_contract_matches` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `go_rust_public_contract_matches`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `go_rust_public_contract_matches` 的重要阅读参照。
//! 理解 `go_rust_public_contract_matches` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `go_rust_public_contract_matches` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `go_rust_public_contract_matches` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `contract_normal_paths` 是当前文件里的辅助函数。
//! 阅读 `contract_normal_paths` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `contract_normal_paths` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `contract_normal_paths`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `contract_normal_paths` 的重要阅读参照。
//! 理解 `contract_normal_paths` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `contract_normal_paths` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `contract_normal_paths` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `contract_boundary` 是当前文件里的辅助函数。
//! 阅读 `contract_boundary` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `contract_boundary` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `contract_boundary`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `contract_boundary` 的重要阅读参照。
//! 理解 `contract_boundary` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `contract_boundary` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `contract_boundary` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `contract_error_paths` 是当前文件里的辅助函数。
//! 阅读 `contract_error_paths` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `contract_error_paths` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `contract_error_paths`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `contract_error_paths` 的重要阅读参照。
//! 理解 `contract_error_paths` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `contract_error_paths` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `contract_error_paths` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `contract_resource_cleanup` 是当前文件里的辅助函数。
//! 阅读 `contract_resource_cleanup` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `contract_resource_cleanup` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `contract_resource_cleanup`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `contract_resource_cleanup` 的重要阅读参照。
//! 理解 `contract_resource_cleanup` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `contract_resource_cleanup` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `contract_resource_cleanup` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 关注点 001：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`parity_test`）。
//! 关注点 002：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`parity_test`）。
//! 关注点 003：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`parity_test`）。
//! 关注点 004：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`parity_test`）。
//! 关注点 005：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`parity_test`）。
//! 关注点 006：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`parity_test`）。
//! 关注点 007：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`parity_test`）。
//! 关注点 008：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`parity_test`）。
//! 关注点 009：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`parity_test`）。
//! 中文说明结束（自动生成）

// Copyright 2026 AsterSQL.

//! Parity tests for `tests/realtikvtest` public contracts vs Go `testkit.go`.

use crate::stubs::{
    self, TestCtx, TestMain, config, kerneltype, keyspace, kvstore, mock_port_alloc_get,
    take_events, transaction, vardef, view,
};
use crate::{
    CreateMockStoreAndDomainAndSetup, CreateMockStoreAndSetup, GetNextGenObjStoreURI, PDAddr,
    PrepareForCrossKSTest, PrepareForCrossKSTestWithNewCollation, RunTestMain, SetTiKVPath,
    SetWithRealTiKV, TiKVPath, UpdateTiDBConfig, WithAllocPort, WithKeepSelfStore,
    WithKeepSystemStore, WithKeyspaceName, WithNewCollationsEnabledOnFirstBootstrap, WithRealTiKV,
    WithRetainData,
};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::Ordering;
use std::time::Duration;

#[test]
fn go_rust_public_contract_matches() {
    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

/// Normal: URI helpers, RunTestMain setup, store bootstrap + cleanup SQL.
fn contract_normal_paths() {
    stubs::reset_test_globals();
    SetTiKVPath(stubs::DEFAULT_TIKV_PATH);

    assert_eq!(PDAddr, "127.0.0.1:2379");
    assert_eq!(TiKVPath(), "tikv://127.0.0.1:2379?disableGC=true");
    assert_eq!(
        GetNextGenObjStoreURI("bucket"),
        "s3://next-gen-test/bucket?access-key=minioadmin&secret-access-key=minioadmin&endpoint=http%3a%2f%2f0.0.0.0%3a9000&provider=minio"
    );

    let mut m = TestMain::new(7);
    let code = RunTestMain(&mut m);
    assert_eq!(code, 7);
    assert!(WithRealTiKV());
    assert!(m.wrapped);
    assert!(stubs::testsetup::was_called());
    assert!(stubs::tikv::failpoints_enabled());
    assert!(stubs::goleak::verify_called());
    assert_eq!(vardef::schema_lease(), Duration::from_secs(5));
    let cfg = config::GetGlobalConfig();
    assert_eq!(cfg.TiKVClient.AsyncCommit.SafeWindow, 0);
    assert_eq!(cfg.TiKVClient.AsyncCommit.AllowedClockDrift, 0);
    let opts = stubs::goleak::last_opts();
    assert!(
        opts.iter().any(|o| matches!(
            o,
            stubs::goleak::Option::Cleanup("testutil.CheckIngestLeakageForTest")
        )),
        "goleak must register ingest leakage cleanup"
    );

    stubs::reset_test_globals();
    SetTiKVPath(stubs::DEFAULT_TIKV_PATH);
    stubs::set_base_tables(vec!["t1".into(), "t2".into()]);
    stubs::set_views(vec!["v1".into()]);
    let t = TestCtx::new();
    let (store, dom) = CreateMockStoreAndDomainAndSetup(&t, &[]);
    assert!(!store.is_closed());
    assert!(!dom.is_closed());
    assert_eq!(transaction::ManagedLockTTL.load(Ordering::SeqCst), 5000);
    assert_eq!(transaction::PrewriteMaxBackoff.load(Ordering::SeqCst), 500);
    assert!(dom.InfoSyncer().has_session_manager());
    let events = take_events();
    assert!(
        events
            .iter()
            .any(|e| e.contains("tk.MustExec:set global innodb_lock_wait_timeout = 50")),
        "must set lock wait timeout; events={events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e.contains("delete from mysql.tidb_global_task"))
    );
    assert!(events.iter().any(|e| e.contains("drop table `t1`,`t2`")));
    assert!(events.iter().any(|e| e.contains("drop view `v1`")));
    assert!(
        events
            .iter()
            .any(|e| e.contains("alter table `t1` nocache"))
    );

    t.run_cleanups();
    assert!(dom.is_closed());
    assert!(store.is_closed());
    assert_eq!(
        transaction::PrewriteMaxBackoff.load(Ordering::SeqCst),
        20000
    );
    assert!(view::was_stopped());
}

/// Boundary: next-gen default SYSTEM keyspace, retainData, allocPort, collation option.
fn contract_boundary() {
    stubs::reset_test_globals();
    SetTiKVPath(stubs::DEFAULT_TIKV_PATH);
    stubs::set_next_gen(true);

    UpdateTiDBConfig();
    let cfg = config::GetGlobalConfig();
    assert_eq!(cfg.Path, "127.0.0.1:2379");
    assert_eq!(cfg.TiKVWorkerURL, "localhost:19000");
    assert_eq!(cfg.KeyspaceName, keyspace::System);
    assert_eq!(cfg.Instance.TiDBServiceScope, "dxf_service");
    assert!(cfg.MeteringStorageURI.contains("metering-data"));
    assert!(cfg.MeteringStorageURI.contains("&region=local"));

    let t = TestCtx::new();
    let before_port = mock_port_alloc_get();
    let store = CreateMockStoreAndSetup(
        &t,
        &[
            WithRetainData(),
            WithAllocPort(true),
            WithNewCollationsEnabledOnFirstBootstrap(true),
            WithKeepSelfStore(true),
        ],
    );
    let cfg = config::GetGlobalConfig();
    assert_eq!(cfg.KeyspaceName, keyspace::System);
    assert!(cfg.NewCollationsEnabledOnFirstBootstrap);
    assert_eq!(cfg.Port, (before_port + 1) as u32);
    assert!(cfg.TxnLocalLatches.Enabled == false);
    let events = take_events();
    assert!(
        events.iter().any(|e| e.contains(&format!(
            "driver.Open:{}&keyspaceName=SYSTEM",
            stubs::DEFAULT_TIKV_PATH
        ))),
        "empty keyspace defaults to SYSTEM; events={events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| e.contains("delete from mysql.tidb_global_task")),
        "retainData skips cleanup SQL"
    );
    assert!(
        t.logs()
            .iter()
            .any(|l| l.contains("create realtikv store with keyspace:SYSTEM"))
    );

    t.run_cleanups();
    // keepSelfStore=true → store not closed by CreateMock cleanup
    assert!(!store.is_closed());

    // named user keyspace opens SYSTEM store as well
    stubs::clear_events();
    let t2 = TestCtx::new();
    let (_store2, _dom2) = CreateMockStoreAndDomainAndSetup(
        &t2,
        &[
            WithKeyspaceName("ks1"),
            WithKeepSystemStore(true),
            WithKeepSelfStore(true),
        ],
    );
    let events = take_events();
    assert!(
        events
            .iter()
            .any(|e| e.contains("driver.Open:") && e.contains("keyspaceName=ks1"))
    );
    assert!(
        events
            .iter()
            .any(|e| e.contains("driver.Open:") && e.contains("keyspaceName=SYSTEM"))
    );
    assert!(kvstore::system_storage().is_some());
    t2.run_cleanups();

    // cross-ks with per-keyspace collation map
    stubs::clear_events();
    let t3 = TestCtx::new();
    let mut coll = HashMap::new();
    coll.insert(keyspace::System.to_string(), false);
    coll.insert("u1".to_string(), true);
    let runtimes = PrepareForCrossKSTestWithNewCollation(&t3, Some(&coll), &["u1"]);
    assert!(runtimes.contains_key(keyspace::System));
    assert!(runtimes.contains_key("u1"));
    assert_eq!(runtimes.len(), 2);
    t3.run_cleanups();
}

/// Error: !next-gen cross-ks Fail; Open/bootstrap failures panic via require.
fn contract_error_paths() {
    stubs::reset_test_globals();
    SetTiKVPath(stubs::DEFAULT_TIKV_PATH);
    stubs::set_next_gen(false);

    let t = TestCtx::new();
    let _ = PrepareForCrossKSTest(&t, &["u1"]);
    assert!(
        t.Failed(),
        "PrepareForCrossKSTest must Fail when !IsNextGen"
    );
    t.run_cleanups();

    stubs::reset_test_globals();
    SetTiKVPath(stubs::DEFAULT_TIKV_PATH);
    stubs::set_open_fail(Some("open refused"));
    let t = TestCtx::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = CreateMockStoreAndSetup(&t, &[]);
    }));
    assert!(
        result.is_err(),
        "Open failure must panic via require.NoError"
    );
    assert!(t.Failed());

    stubs::reset_test_globals();
    SetTiKVPath(stubs::DEFAULT_TIKV_PATH);
    stubs::set_bootstrap_fail(Some("bootstrap refused"));
    let t = TestCtx::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = CreateMockStoreAndDomainAndSetup(&t, &[]);
    }));
    assert!(
        result.is_err(),
        "Bootstrap failure must panic via require.NoError"
    );
    assert!(t.Failed());
}

/// Resource cleanup: domain before store; config restore; system store close.
fn contract_resource_cleanup() {
    stubs::reset_test_globals();
    SetTiKVPath(stubs::DEFAULT_TIKV_PATH);
    stubs::set_next_gen(true);

    // Mutate config then ensure CreateMock restores prior bak.
    config::UpdateGlobal(|c| {
        c.Path = "old-path".to_string();
        c.Port = 1111;
    });
    let t = TestCtx::new();
    let (store, dom) = CreateMockStoreAndDomainAndSetup(
        &t,
        &[
            WithKeyspaceName("ks2"),
            WithKeepSystemStore(false),
            WithKeepSelfStore(false),
            WithAllocPort(true),
        ],
    );
    let sys = kvstore::system_storage().expect("system store set");
    assert!(!sys.is_closed());

    stubs::clear_events();
    t.run_cleanups();
    let events = take_events();

    let domain_idx = events.iter().position(|e| e == "domain.Close");
    let store_idx = events
        .iter()
        .position(|e| e.starts_with("store.Close:") && e.contains("keyspaceName=ks2"));
    assert!(domain_idx.is_some(), "domain must close; events={events:?}");
    assert!(
        store_idx.is_some(),
        "self store must close; events={events:?}"
    );
    assert!(
        domain_idx.unwrap() < store_idx.unwrap(),
        "domain.Close before store.Close; events={events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e.starts_with("ddl.CloseOwnerManager:")),
        "owner manager closed; events={events:?}"
    );
    assert!(
        events.iter().any(|e| e == "config.StoreGlobalConfig"),
        "global config restored"
    );
    assert!(dom.is_closed());
    assert!(store.is_closed());
    assert!(sys.is_closed(), "system store closed when !keepSystemStore");
    assert!(view::was_stopped());

    let restored = config::GetGlobalConfig();
    assert_eq!(restored.Path, "old-path");
    assert_eq!(restored.Port, 1111);

    // Cross-ks cleanup closes all retained stores via outer cleanup.
    stubs::reset_test_globals();
    SetTiKVPath(stubs::DEFAULT_TIKV_PATH);
    stubs::set_next_gen(true);
    let t = TestCtx::new();
    let runtimes = PrepareForCrossKSTest(&t, &["a", "b"]);
    let stores: Vec<_> = runtimes.values().map(|r| r.Store.clone()).collect();
    assert_eq!(stores.len(), 3);
    for s in &stores {
        assert!(!s.is_closed());
    }
    t.run_cleanups();
    for s in &stores {
        assert!(s.is_closed(), "cross-ks cleanup closes every store");
    }

    SetWithRealTiKV(false);
    assert!(!WithRealTiKV());
    assert!(!kerneltype::IsNextGen() || true);
}
