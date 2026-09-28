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

// Aster 侧 stmtstats/聚合器语义单测：KV 去重、RU 增量、版本切换与 key 上限。

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use super::*;

/// 构造可变的共享 RUDetails（读写/TiKV v2/TiFlash 分量）。
fn ru_details(read: f64, write: f64, tikv_v2: f64, tiflash: f64) -> SharedRUDetails {
    Arc::new(RwLock::new(execdetails::RUDetails {
        read_ru: read,
        write_ru: write,
        tikv_ru_v2: tikv_v2,
        tiflash_ru: tiflash,
        ..Default::default()
    }))
}

/// 语句网络字节/耗时与 KvExecCounter 去重、开关关闭时不计次，与 Go 一致。
#[test]
fn statement_stats_and_kv_counter_match_go_semantics() {
    let _serial = super::test_support::stmtstats_guard();
    let stats = StatementStats::new();

    stats.OnExecutionBegin(
        b"sql",
        b"plan",
        Some(&ExecBeginInfo {
            InNetworkBytes: 11,
            ..ExecBeginInfo::default()
        }),
    );
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&ExecFinishInfo {
            OutNetworkBytes: 13,
            ExecDuration: SignedDuration::from_nanos(2_000_000),
            ..ExecFinishInfo::default()
        }),
    );

    topsql_state::EnableTopSQL();
    let counter = stats.CreateKvExecCounter(b"sql", b"plan");
    counter.record_target("tikv-1");
    counter.record_target("tikv-1");
    topsql_state::DisableTopSQL();
    counter.record_target("tikv-2");
    topsql_state::EnableTopSQL();
    counter.record_target("tikv-2");
    let response = counter
        .intercept("tikv-3", 41_u64, |target, request| {
            assert_eq!(target, "tikv-3");
            Ok::<_, ()>(request + 1)
        })
        .expect("next interceptor result");
    assert_eq!(response, 42);
    topsql_state::DisableTopSQL();

    let data = stats.Take();
    let item = data
        .get(&SQLPlanDigest::new(b"sql", b"plan"))
        .expect("statement item");
    assert_eq!(item.ExecCount, 1);
    assert_eq!(item.SumDurationNs, 2_000_000);
    assert_eq!(item.DurationCount, 1);
    assert_eq!(item.NetworkInBytes, 11);
    assert_eq!(item.NetworkOutBytes, 13);
    let kv_counts = item
        .KvStatsItem
        .KvExecCount
        .as_ref()
        .expect("initialized KV count map");
    assert_eq!(kv_counts.get("tikv-1"), Some(&1));
    assert_eq!(kv_counts.get("tikv-2"), Some(&1));
    assert_eq!(kv_counts.get("tikv-3"), Some(&1));
    assert!(stats.Take().is_empty());
}

/// RUv1：多次 Merge 只在 Begin 计 ExecCount=1，后续为增量且 ExecCount=0。
#[test]
fn ru_v1_multi_tick_and_finish_preserve_begin_based_count() {
    let _serial = super::test_support::stmtstats_guard();
    let stats = StatementStats::new();
    let ru = ru_details(0.0, 0.0, 0.0, 0.0);
    let key = RUKey::new("u1", b"sql", b"plan");

    stats.OnExecutionBegin(
        b"sql",
        b"plan",
        Some(&ExecBeginInfo {
            User: "u1".to_owned(),
            RUDetails: Some(ru.clone()),
            RUVersion: RU_VERSION_V1,
            TopRUEnabled: true,
            ..ExecBeginInfo::default()
        }),
    );

    ru.write().expect("RU lock poisoned").read_ru = 10.0;
    let first = stats.MergeRUInto();
    assert_eq!(first.get(&key).expect("first RU").TotalRU, 10.0);
    assert_eq!(first.get(&key).expect("first RU").ExecCount, 1);

    ru.write().expect("RU lock poisoned").read_ru = 25.0;
    let second = stats.MergeRUInto();
    assert_eq!(second.get(&key).expect("second RU").TotalRU, 15.0);
    assert_eq!(second.get(&key).expect("second RU").ExecCount, 0);

    ru.write().expect("RU lock poisoned").read_ru = 33.0;
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&ExecFinishInfo {
            RUDetails: Some(ru),
            User: "u1".to_owned(),
            ExecDuration: SignedDuration::from_nanos(5_000_000_000),
            TopRUEnabled: true,
            ..ExecFinishInfo::default()
        }),
    );
    let tail = stats.MergeRUInto();
    let tail = tail.get(&key).expect("tail RU");
    assert_eq!(tail.TotalRU, 8.0);
    assert_eq!(tail.ExecCount, 0);
    assert_eq!(tail.ExecDuration, 5_000_000_000);
    assert!(stats.MergeRUInto().is_empty());
}

/// 测试用语句收集器：合并收到的 StatementStatsMap。
#[derive(Default)]
struct StmtCollector(Mutex<StatementStatsMap>);

impl Collector for StmtCollector {
    fn CollectStmtStatsMap(&self, data: StatementStatsMap) {
        self.0.lock().expect("collector lock poisoned").Merge(data);
    }
}

/// 测试用 RU 收集器：合并增量并记录版本与切换事件。
#[derive(Default)]
struct RuCollector(Mutex<(RUIncrementMap, RUVersion, Vec<RUVersion>)>);

impl RUCollector for RuCollector {
    fn CollectRUIncrements(&self, data: RUIncrementMap, version: RUVersion) {
        let mut collected = self.0.lock().expect("collector lock poisoned");
        collected.0.Merge(data);
        collected.1 = version;
    }

    fn OnRUVersionChange(&self, version: RUVersion) {
        self.0
            .lock()
            .expect("collector lock poisoned")
            .2
            .push(version);
    }
}

/// aggregate_all 先排空 RU 再注销 Finished 会话，尾部 RU 不丢失。
#[test]
fn aggregator_drains_ru_before_unregistering_finished_stats() {
    let _serial = super::test_support::stmtstats_guard();
    init_reporter_metrics_for_tests();
    topsql_state::EnableTopSQL();
    topsql_state::EnableTopRU();

    let aggregator = Aggregator::new();
    let stmt_collector = Arc::new(StmtCollector::default());
    let ru_collector = Arc::new(RuCollector::default());
    aggregator.register_collector(stmt_collector.clone());
    aggregator.register_ru_collector(ru_collector.clone());

    let stats = Arc::new(StatementStats::new());
    let ru = ru_details(7.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sql",
        b"plan",
        Some(&ExecBeginInfo {
            User: "u1".to_owned(),
            RUDetails: Some(ru.clone()),
            TopRUEnabled: true,
            ..ExecBeginInfo::default()
        }),
    );
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&ExecFinishInfo {
            RUDetails: Some(ru),
            User: "u1".to_owned(),
            ExecDuration: SignedDuration::from_nanos(1_000_000_000),
            TopRUEnabled: true,
            ..ExecFinishInfo::default()
        }),
    );
    stats.SetFinished();
    aggregator.register(stats);

    aggregator.aggregate_all();

    let stmt = stmt_collector.0.lock().expect("collector lock poisoned");
    assert_eq!(stmt.len(), 1);
    drop(stmt);
    let ru = ru_collector.0.lock().expect("collector lock poisoned");
    let key = RUKey::new("u1", b"sql", b"plan");
    assert_eq!(ru.0.get(&key).expect("collected RU").TotalRU, 7.0);
    assert_eq!(ru.0.get(&key).expect("collected RU").ExecCount, 1);
    assert_eq!(aggregator.stats_len(), 0);

    topsql_state::DisableTopSQL();
    topsql_state::DisableTopRU();
}

/// 负耗时：语句 ExecCount 仍计，Duration 不计；RU 保留 Begin 计数、TotalRU=0。
#[test]
fn negative_duration_clears_ru_context_without_duration_stats() {
    let _serial = super::test_support::stmtstats_guard();
    let stats = StatementStats::new();
    let ru = ru_details(5.0, 0.0, 0.0, 0.0);
    let key = RUKey::new("u1", b"sql", b"plan");
    stats.OnExecutionBegin(
        b"sql",
        b"plan",
        Some(&ExecBeginInfo {
            User: "u1".to_owned(),
            RUDetails: Some(ru.clone()),
            TopRUEnabled: true,
            ..ExecBeginInfo::default()
        }),
    );
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&ExecFinishInfo {
            User: "u1".to_owned(),
            RUDetails: Some(ru),
            ExecDuration: SignedDuration::from_nanos(-1),
            TopRUEnabled: true,
            ..ExecFinishInfo::default()
        }),
    );

    let item = stats
        .Take()
        .remove(&SQLPlanDigest::new(b"sql", b"plan"))
        .expect("statement item");
    assert_eq!(item.ExecCount, 1);
    assert_eq!(item.DurationCount, 0);
    assert_eq!(item.SumDurationNs, 0);
    let ru = stats.MergeRUInto();
    assert_eq!(ru.get(&key).expect("begin count").ExecCount, 1);
    assert_eq!(ru.get(&key).expect("begin count").TotalRU, 0.0);
    assert!(stats.MergeRUInto().is_empty());
}

/// 可原子切换的测试用 RU 版本提供者。
struct VersionProvider(AtomicI32);

impl RUVersionProvider for VersionProvider {
    fn GetRUVersion(&self) -> RUVersion {
        self.0.load(Ordering::SeqCst)
    }
}

/// 版本切换时通知收集器并丢弃旧版本尾部 RU，不混入新版本。
#[test]
fn aggregator_resets_ru_state_on_version_handover() {
    let _serial = super::test_support::stmtstats_guard();
    topsql_state::EnableTopRU();
    let provider = Arc::new(VersionProvider(AtomicI32::new(RU_VERSION_V1)));
    let aggregator = Aggregator::new();
    aggregator.set_ru_version_provider(Some(provider.clone()));
    let collector = Arc::new(RuCollector::default());
    aggregator.register_ru_collector(collector.clone());
    let stats = Arc::new(StatementStats::new());
    aggregator.register(stats.clone());

    let ru = ru_details(4.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sql",
        b"plan",
        Some(&ExecBeginInfo {
            User: "u1".to_owned(),
            RUDetails: Some(ru.clone()),
            RUVersion: RU_VERSION_V1,
            TopRUEnabled: true,
            ..ExecBeginInfo::default()
        }),
    );
    aggregator.drain_and_push_ru();
    assert_eq!(
        collector
            .0
            .lock()
            .expect("collector lock poisoned")
            .0
            .get(&RUKey::new("u1", b"sql", b"plan"))
            .expect("first version RU")
            .TotalRU,
        4.0
    );

    ru.write().expect("RU lock poisoned").read_ru = 9.0;
    provider.0.store(RU_VERSION_V2, Ordering::SeqCst);
    aggregator.drain_and_push_ru();
    let collected = collector.0.lock().expect("collector lock poisoned");
    assert_eq!(collected.2, vec![RU_VERSION_V2]);
    assert_eq!(
        collected.0.len(),
        1,
        "handover must not emit old-version tail RU"
    );
    drop(collected);

    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&ExecFinishInfo {
            User: "u1".to_owned(),
            RUDetails: Some(ru),
            ExecDuration: SignedDuration::from_nanos(1_000),
            TopRUEnabled: true,
            ..ExecFinishInfo::default()
        }),
    );
    assert!(stats.MergeRUInto().is_empty());
    topsql_state::DisableTopRU();
}

/// start/close 重复调用幂等。
#[test]
fn aggregator_lifecycle_is_idempotent() {
    let _serial = super::test_support::stmtstats_guard();
    let aggregator = Aggregator::new();
    for _ in 0..3 {
        aggregator.start();
        aggregator.start();
        assert!(!aggregator.closed());
        aggregator.close();
        aggregator.close();
        assert!(aggregator.closed());
    }
}

/// RUv2 使用 TiKV/TiFlash 分量合计 TotalRU（此处 11+3=14）。
#[test]
fn ru_v2_uses_tikv_tiflash_and_tidb_metric_totals() {
    let _serial = super::test_support::stmtstats_guard();
    let stats = StatementStats::new();
    let ru = ru_details(100.0, 200.0, 11.0, 3.0);
    let key = RUKey::new("u2", b"sql-v2", b"plan-v2");
    stats.OnExecutionBegin(
        b"sql-v2",
        b"plan-v2",
        Some(&ExecBeginInfo {
            User: "u2".to_owned(),
            RUDetails: Some(ru),
            RUV2Metrics: Some(Arc::new(execdetails::RUV2Metrics::default())),
            RUV2Weights: execdetails::RUV2Weights::default(),
            RUVersion: RU_VERSION_V2,
            TopRUEnabled: true,
            ..ExecBeginInfo::default()
        }),
    );
    let data = stats.MergeRUInto();
    assert_eq!(data.get(&key).expect("RU v2 increment").TotalRU, 14.0);
    assert_eq!(data.get(&key).expect("RU v2 increment").ExecCount, 1);
}

/// 超过 10k distinct RU key 时裁剪并累加丢弃指标。
#[test]
fn aggregator_caps_distinct_ru_keys_and_records_drops() {
    let _serial = super::test_support::stmtstats_guard();
    init_reporter_metrics_for_tests();
    topsql_state::EnableTopRU();
    let aggregator = Aggregator::new();
    let collector = Arc::new(RuCollector::default());
    aggregator.register_ru_collector(collector.clone());
    let (before_keys, before_ru) = reporter_drop_metrics_for_tests();

    for index in 0..10_001 {
        let stats = Arc::new(StatementStats::new());
        let sql = format!("sql-{index}");
        stats.OnExecutionBegin(
            sql.as_bytes(),
            b"plan",
            Some(&ExecBeginInfo {
                User: format!("user-{index}"),
                RUDetails: Some(ru_details(1.0, 0.0, 0.0, 0.0)),
                TopRUEnabled: true,
                ..ExecBeginInfo::default()
            }),
        );
        aggregator.register(stats);
    }
    aggregator.drain_and_push_ru();

    let collected = collector.0.lock().expect("collector lock poisoned");
    assert_eq!(collected.0.len(), 10_000);
    drop(collected);
    let (after_keys, after_ru) = reporter_drop_metrics_for_tests();
    let dropped_keys = after_keys - before_keys;
    let dropped_ru = after_ru - before_ru;
    assert_eq!(dropped_keys, 1.0);
    assert_eq!(dropped_ru, 1.0);
    topsql_state::DisableTopRU();
}
