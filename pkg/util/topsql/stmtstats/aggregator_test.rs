// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Aggregator 集成单测：全局生命周期、开关门控、RU 版本切换、并发尾部增量与 key 上限。

#![allow(non_snake_case)]

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use super::stmtstats_tests::*;

/// 一秒对应的纳秒数，用于构造执行耗时。
const SECOND_NS: i64 = 1_000_000_000;
/// 与生产聚合器一致的单次 RU key 上限。
const MAX_RU_KEYS_PER_AGGREGATE: usize = 10_000;

/// 取得 stmtstats 测试串行锁。
fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    super::test_support::stmtstats_guard()
}

/// 向 StatementStats 写入一条已完成的 RU 样本（Begin+Finish）。
fn add_finished_ru(stats: &Arc<StatementStats>, key: &RUKey, total_ru: f64, duration_ns: i64) {
    let details = ru_details(total_ru, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        key.SQLDigest.as_bytes(),
        key.PlanDigest.as_bytes(),
        Some(&ExecBeginInfo {
            RUDetails: Some(details.clone()),
            User: key.User.clone(),
            RUVersion: RU_VERSION_V1,
            TopRUEnabled: true,
            ..Default::default()
        }),
    );
    stats.OnExecutionFinished(
        key.SQLDigest.as_bytes(),
        key.PlanDigest.as_bytes(),
        Some(&ExecFinishInfo {
            RUDetails: Some(details),
            User: key.User.clone(),
            ExecDuration: SignedDuration::from_nanos(duration_ns),
            TopRUEnabled: true,
            ..Default::default()
        }),
    );
}

/// Setup/Close 全局聚合器可重复且状态一致。
#[test]
fn TestSetupCloseAggregator() {
    let _guard = test_guard();
    CloseAggregator();
    for _ in 0..3 {
        SetupAggregator();
        assert!(!global_aggregator().closed());
        CloseAggregator();
        assert!(global_aggregator().closed());
    }
}

/// Close 后仍可绑定 RU 版本提供者，再 Setup 时版本生效。
#[test]
fn TestBindRUVersionProviderAfterCloseAggregator() {
    let _guard = test_guard();
    CloseAggregator();
    let provider = Arc::new(TestRUVersionProvider::new(RU_VERSION_V2));
    BindRUVersionProvider(Some(provider));
    SetupAggregator();
    assert!(!global_aggregator().closed());
    assert_eq!(global_aggregator().current_ru_version(), RU_VERSION_V2);
    CloseAggregator();
    assert!(global_aggregator().closed());
    assert_eq!(global_aggregator().current_ru_version(), RU_VERSION_V2);
    BindRUVersionProvider(None);
    assert_eq!(global_aggregator().current_ru_version(), DEFAULT_RU_VERSION);
}

/// 注册/注销语句 Collector 后仅在注册期间收到批次。
#[test]
fn TestRegisterUnregisterCollector() {
    let _guard = test_guard();
    CloseAggregator();
    reset_top_state();
    topsql_state::EnableTopSQL();
    let concrete = Arc::new(StatementCollector::default());
    let collector: Arc<dyn Collector> = concrete.clone();
    RegisterCollector(collector.clone());
    let stats = CreateStatementStats();
    stats.OnExecutionBegin(b"sql-1", b"", Some(&ExecBeginInfo::default()));
    global_aggregator().drain_and_push_stmt_stats();
    assert_eq!(concrete.batches.lock().unwrap().len(), 1);
    UnregisterCollector(&collector);
    let stats = CreateStatementStats();
    stats.OnExecutionBegin(b"sql-2", b"", Some(&ExecBeginInfo::default()));
    global_aggregator().drain_and_push_stmt_stats();
    assert_eq!(concrete.batches.lock().unwrap().len(), 1);
    reset_top_state();
}

/// 注册/注销 RUCollector；Setup 同步 last RU version 后再测流量。
#[test]
fn TestRegisterUnregisterRUCollector() {
    let _guard = test_guard();
    CloseAggregator();
    reset_top_state();
    topsql_state::EnableTopRU();
    // Setup synchronizes the global aggregator's last RU version with the
    // provider, exactly as the Go package does before collector traffic starts.
    SetupAggregator();
    CloseAggregator();
    let concrete = Arc::new(TestRUCollector::default());
    let collector: Arc<dyn RUCollector> = concrete.clone();
    RegisterRUCollector(collector.clone());
    let stats = CreateStatementStats();
    add_finished_ru(&stats, &RUKey::new("u", b"sql-1", b""), 1.0, 1);
    global_aggregator().drain_and_push_ru();
    assert_eq!(concrete.batches.lock().unwrap().len(), 1);
    UnregisterRUCollector(&collector);
    let stats = CreateStatementStats();
    add_finished_ru(&stats, &RUKey::new("u", b"sql-2", b""), 1.0, 1);
    global_aggregator().drain_and_push_ru();
    assert_eq!(concrete.batches.lock().unwrap().len(), 1);
    reset_top_state();
}

/// 本地聚合器注册后 drain 能收集 ExecCount/耗时。
#[test]
fn TestAggregatorRegisterCollect() {
    let _guard = test_guard();
    reset_top_state();
    topsql_state::EnableTopSQL();
    let aggregator = Aggregator::new();
    let stats = Arc::new(StatementStats::new());
    aggregator.register(stats.clone());
    stats.OnExecutionBegin(b"SQL-1", b"", Some(&ExecBeginInfo::default()));
    stats.OnExecutionFinished(
        b"SQL-1",
        b"",
        Some(&ExecFinishInfo {
            ExecDuration: SignedDuration::from_nanos(1_000_000),
            ..Default::default()
        }),
    );
    let collector = Arc::new(StatementCollector::default());
    aggregator.register_collector(collector.clone());
    aggregator.drain_and_push_stmt_stats();
    let batches = collector.batches.lock().unwrap();
    let item = &batches[0][&SQLPlanDigest::new(b"SQL-1", b"")];
    assert_eq!(item.ExecCount, 1);
    assert_eq!(item.SumDurationNs, 1_000_000);
    reset_top_state();
}

/// start/close 随机交错后最终可关闭。
#[test]
fn TestAggregatorRunClose() {
    let _guard = test_guard();
    let aggregator = Aggregator::new();
    assert!(aggregator.closed());
    aggregator.start();
    assert!(!aggregator.closed());
    aggregator.close();
    assert!(aggregator.closed());
    let mut random = 1_u64;
    for _ in 0..100 {
        random = random
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        if random & 1 == 0 {
            aggregator.start();
        } else {
            aggregator.close();
        }
    }
    aggregator.close();
    assert!(aggregator.closed());
}

/// TopSQL 关闭时不推送；开启后才向 Collector 发批次。
#[test]
fn TestAggregatorDisableAggregate() {
    let _guard = test_guard();
    reset_top_state();
    let aggregator = Aggregator::new();
    let collector = Arc::new(StatementCollector::default());
    aggregator.register_collector(collector.clone());
    let disabled = Arc::new(StatementStats::new());
    disabled.OnExecutionBegin(b"", b"", Some(&ExecBeginInfo::default()));
    aggregator.register(disabled.clone());
    aggregator.drain_and_push_stmt_stats();
    assert!(disabled.Take().is_empty());
    assert!(collector.batches.lock().unwrap().is_empty());

    topsql_state::EnableTopSQL();
    let enabled = Arc::new(StatementStats::new());
    enabled.OnExecutionBegin(b"", b"", Some(&ExecBeginInfo::default()));
    aggregator.register(enabled.clone());
    aggregator.drain_and_push_stmt_stats();
    assert!(enabled.Take().is_empty());
    assert_eq!(collector.batches.lock().unwrap().len(), 1);
    reset_top_state();
}

/// TopRU 关闭时 Merge 清空本地增量但不向 RUCollector 推送。
#[test]
fn TestAggregatorDisableAggregateRUNoEmit() {
    let _guard = test_guard();
    reset_top_state();
    let aggregator = Aggregator::new();
    let stats = Arc::new(StatementStats::new());
    add_finished_ru(&stats, &RUKey::new("u1", b"s1", b""), 1.0, 1);
    aggregator.register(stats.clone());
    let collector = Arc::new(TestRUCollector::default());
    aggregator.register_ru_collector(collector.clone());
    aggregator.drain_and_push_ru();
    assert!(stats.MergeRUInto().is_empty());
    assert!(collector.batches.lock().unwrap().is_empty());
}

/// Finished 会话在 aggregate_all 中先推 RU 再注销，尾部 RU 保留。
#[test]
fn TestAggregatorRunOrderKeepsFinishedRU() {
    let _guard = test_guard();
    reset_top_state();
    topsql_state::EnableTopRU();
    let aggregator = Aggregator::new();
    let stats = Arc::new(StatementStats::new());
    let key = RUKey::new("u1", b"s1", b"");
    add_finished_ru(&stats, &key, 1.0, 1);
    stats.SetFinished();
    aggregator.register(stats);
    let collector = Arc::new(TestRUCollector::default());
    aggregator.register_ru_collector(collector.clone());
    aggregator.aggregate_all();
    let batches = collector.batches.lock().unwrap();
    assert_eq!(batches[0].0[&key].TotalRU, 1.0);
    assert_eq!(aggregator.stats_len(), 0);
    reset_top_state();
}

/// RU 版本切换触发 OnRUVersionChange，丢弃旧版本尾部后再收新版本增量。
#[test]
fn TestAggregatorDetectsRUVersionHandover() {
    let _guard = test_guard();
    reset_top_state();
    topsql_state::EnableTopRU();
    let provider = Arc::new(TestRUVersionProvider::new(RU_VERSION_V1));
    let aggregator = Aggregator::new();
    aggregator.set_ru_version_provider(Some(provider.clone()));
    let stats = Arc::new(StatementStats::new());
    let key = RUKey::new("u1", b"sql1", b"plan1");
    let first = ru_details(10.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sql1",
        b"plan1",
        Some(&ExecBeginInfo {
            RUDetails: Some(first),
            User: "u1".to_owned(),
            RUVersion: RU_VERSION_V1,
            TopRUEnabled: true,
            ..Default::default()
        }),
    );
    aggregator.register(stats.clone());
    let collector = Arc::new(TestRUCollector::default());
    aggregator.register_ru_collector(collector.clone());
    aggregator.drain_and_push_ru();
    assert_eq!(collector.batches.lock().unwrap()[0].1, RU_VERSION_V1);

    let stale = ru_details(5.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sql1",
        b"plan1",
        Some(&ExecBeginInfo {
            RUDetails: Some(stale),
            User: "u1".to_owned(),
            RUVersion: RU_VERSION_V1,
            TopRUEnabled: true,
            ..Default::default()
        }),
    );
    provider.set(RU_VERSION_V2);
    aggregator.drain_and_push_ru();
    assert_eq!(*collector.changes.lock().unwrap(), vec![RU_VERSION_V2]);
    assert_eq!(collector.batches.lock().unwrap().len(), 1);
    assert!(stats.MergeRUInto().is_empty());

    let current = ru_details(0.0, 0.0, 7.0, 0.0);
    stats.OnExecutionBegin(
        b"sql1",
        b"plan1",
        Some(&ExecBeginInfo {
            RUDetails: Some(current),
            User: "u1".to_owned(),
            RUVersion: RU_VERSION_V2,
            TopRUEnabled: true,
            ..Default::default()
        }),
    );
    aggregator.drain_and_push_ru();
    let batches = collector.batches.lock().unwrap();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[1].1, RU_VERSION_V2);
    assert_eq!(batches[1].0[&key].TotalRU, 7.0);
    reset_top_state();
}

/// TopSQL/TopRU 开关组合矩阵：各自独立门控语句与 RU 推送。
#[test]
fn TestAggregatorTopSQLTopRUCoexistenceMatrix() {
    let _guard = test_guard();
    for (top_sql, top_ru, expect_stmt, expect_ru) in [
        (false, false, false, false),
        (true, false, true, false),
        (false, true, false, true),
        (true, true, true, true),
    ] {
        reset_top_state();
        if top_sql {
            topsql_state::EnableTopSQL();
        }
        if top_ru {
            topsql_state::EnableTopRU();
        }
        let aggregator = Aggregator::new();
        let stats = Arc::new(StatementStats::new());
        let key = RUKey::new("u1", b"sql1", b"plan1");
        add_finished_ru(&stats, &key, 42.0, SECOND_NS);
        aggregator.register(stats);
        let stmt_collector = Arc::new(StatementCollector::default());
        let ru_collector = Arc::new(TestRUCollector::default());
        aggregator.register_collector(stmt_collector.clone());
        aggregator.register_ru_collector(ru_collector.clone());
        aggregator.aggregate_all();
        assert_eq!(
            !stmt_collector.batches.lock().unwrap().is_empty(),
            expect_stmt
        );
        assert_eq!(!ru_collector.batches.lock().unwrap().is_empty(), expect_ru);
        if expect_stmt {
            assert_eq!(
                stmt_collector.batches.lock().unwrap()[0][&SQLPlanDigest::new(b"sql1", b"plan1")]
                    .ExecCount,
                1
            );
        }
        if expect_ru {
            let batches = ru_collector.batches.lock().unwrap();
            assert_eq!(batches[0].0[&key].TotalRU, 42.0);
            assert_eq!(batches[0].0[&key].ExecCount, 1);
        }
    }
    reset_top_state();
}

/// 阻塞式 RUCollector：进入 Collect 后卡住，便于测并发注销。
struct BlockingRUCollector {
    batches: Arc<Mutex<Vec<(RUIncrementMap, RUVersion)>>>,
    entered: Mutex<Sender<()>>,
    release: Mutex<Receiver<()>>,
}

impl RUCollector for BlockingRUCollector {
    fn CollectRUIncrements(&self, increments: RUIncrementMap, version: RUVersion) {
        self.batches.lock().unwrap().push((increments, version));
        self.entered.lock().unwrap().send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
    }

    fn OnRUVersionChange(&self, _version: RUVersion) {}
}

/// 推送进行中并发注销 stats/collector，尾部 RU 仍被本轮批次保留。
#[test]
fn TestAggregatorDrainTailIncrementMatrix() {
    let _guard = test_guard();
    for (tail_ru, concurrent_unregister, concurrent_ru_unregister) in
        [(5.0, false, false), (7.0, true, true)]
    {
        reset_top_state();
        topsql_state::EnableTopRU();
        let aggregator = Aggregator::new();
        let stats = Arc::new(StatementStats::new());
        let key = RUKey::new("u1", b"sql1", b"plan1");
        add_finished_ru(&stats, &key, tail_ru, SECOND_NS);
        stats.SetFinished();
        aggregator.register(stats.clone());
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        let batches = Arc::new(Mutex::new(Vec::new()));
        let concrete = Arc::new(BlockingRUCollector {
            batches: batches.clone(),
            entered: Mutex::new(entered_tx),
            release: Mutex::new(release_rx),
        });
        let collector: Arc<dyn RUCollector> = concrete;
        aggregator.register_ru_collector(collector.clone());
        let worker_aggregator = aggregator.clone();
        let worker = thread::spawn(move || worker_aggregator.aggregate_all());
        entered_rx.recv().unwrap();
        if concurrent_unregister {
            aggregator.unregister(&stats);
        }
        if concurrent_ru_unregister {
            aggregator.unregister_ru_collector(&collector);
        }
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        let batches = batches.lock().unwrap();
        assert_eq!(batches[0].0[&key].TotalRU, tail_ru);
        assert_eq!(batches[0].0[&key].ExecCount, 1);
        assert!(stats.MergeRUInto().is_empty());
        assert_eq!(aggregator.stats_len(), 0);
    }
    reset_top_state();
}

/// 超限 key 被丢弃并计入指标；热 key 合并后仍保留在上限内。
#[test]
fn TestDrainPushRUCapsAtMax() {
    let _guard = test_guard();
    reset_top_state();
    topsql_state::EnableTopRU();
    init_reporter_metrics_for_tests();
    let aggregator = Aggregator::new();
    let hot_key = RUKey::new("hot-user", b"hot-sql", b"hot-plan");
    const TOTAL_DISTINCT_KEYS: usize = MAX_RU_KEYS_PER_AGGREGATE + 51;
    const HOT_RU_PER_SESSION: f64 = 1000.0;
    let low_unique_keys = TOTAL_DISTINCT_KEYS - 1;
    for index in 0..low_unique_keys {
        let stats = Arc::new(StatementStats::new());
        let user = format!("u{index:05}");
        let sql = format!("sql{index:05}");
        add_finished_ru(&stats, &RUKey::new(user, sql.as_bytes(), b"plan"), 1.0, 1);
        add_finished_ru(&stats, &hot_key, HOT_RU_PER_SESSION, 1);
        aggregator.register(stats);
    }
    let collector = Arc::new(TestRUCollector::default());
    aggregator.register_ru_collector(collector.clone());
    let before = reporter_drop_metrics_for_tests();
    aggregator.drain_and_push_ru();
    let after = reporter_drop_metrics_for_tests();
    let batches = collector.batches.lock().unwrap();
    let collected = &batches[0].0;
    assert_eq!(collected.len(), MAX_RU_KEYS_PER_AGGREGATE);
    assert!((after.0 - before.0 - 51.0).abs() < 1e-9);
    assert!((after.1 - before.1 - 51.0).abs() < 1e-9);
    assert!(
        (collected[&hot_key].TotalRU - HOT_RU_PER_SESSION * low_unique_keys as f64).abs() < 1e-9
    );
    reset_top_state();
}
