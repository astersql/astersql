// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// `StatsCacheImpl` 与批量更新缓冲的单元测试。
//
// 覆盖批满自动 flush、健康度分桶指标刷新，以及桶边界索引与 Go 对齐。

use crate::{
    ColumnMeta, IndexColumnMeta, IndexMeta, NewStatsCacheImpl, NewStatsCacheImplForTest,
    RowStatsProvider, StatisticsTable, StatsTableRowCache, TableMeta, buildInTableIDsString,
    newCacheOfBatchUpdate, statsHealthyBucketIndex,
};

#[test]
fn next_check_version_uses_tidb_tso_duration_offset() {
    let cache = cache_with_lease(1_000);
    cache.Put(1, Arc::new(table(1, 10_000_u64 << 18)));
    assert_eq!(cache.GetNextCheckVersionWithOffset(), 5_000_u64 << 18);
}

#[test]
fn cache_update_applies_go_put_then_delete_order() {
    let cache = NewStatsCacheImplForTest().unwrap();
    cache.UpdateStatsCache(&[Arc::new(table(42, 7))], &[42], false);
    assert!(cache.Get(42).is_none());
    assert_eq!(cache.MaxTableStatsVersion(), 7);
}

/// Go helper returns the complete SQL predicate, including the empty-list form.
#[test]
fn build_in_table_ids_string_matches_go_predicate() {
    assert_eq!(buildInTableIDsString(&[7, -2, 9]), "table_id in (7,-2,9)");
    assert_eq!(buildInTableIDsString(&[]), "table_id in ()");
}
use astersql_statistics_handle_metrics::{
    HEALTHY_BUCKET_CONFIGS, StatsHealthyBucketCount, StatsHealthyGauges,
};
use std::collections::HashMap;
use std::sync::Arc;

struct RowProvider;

impl RowStatsProvider for RowProvider {
    fn RowCounts(&self, _: &[i64]) -> Result<HashMap<i64, u64>, crate::CacheError> {
        Ok(HashMap::from([
            (10, 10),
            (20, 8),
            (21, 3),
            (22, 5),
            (30, 12),
        ]))
    }

    fn ColumnLengths(&self, _: &[i64]) -> Result<HashMap<(i64, i64), u64>, crate::CacheError> {
        Ok(HashMap::from([
            ((10, 2), 30),
            ((20, 2), 24),
            ((21, 2), 9),
            ((22, 2), 15),
            ((30, 2), 36),
        ]))
    }
}

#[test]
fn estimate_data_length_matches_go_public_partition_and_sequence_rules() {
    let cache = StatsTableRowCache::default();
    cache
        .UpdateByID(&RowProvider, &[10, 20, 21, 22, 30], true)
        .unwrap();
    let columns = vec![
        ColumnMeta {
            ID: 1,
            FixedLength: Some(4),
            Public: true,
        },
        ColumnMeta {
            ID: 2,
            FixedLength: None,
            Public: true,
        },
        ColumnMeta {
            ID: 3,
            FixedLength: Some(99),
            Public: false,
        },
    ];
    let local = IndexMeta {
        ID: 4,
        Public: true,
        Global: false,
        Columns: vec![
            IndexColumnMeta {
                Offset: 0,
                Length: None,
            },
            IndexColumnMeta {
                Offset: 1,
                Length: Some(2),
            },
        ],
    };
    let hidden = IndexMeta {
        ID: 5,
        Public: false,
        ..Default::default()
    };

    let ordinary = TableMeta {
        ID: 10,
        Columns: columns.clone(),
        Indices: vec![local.clone(), hidden.clone()],
        ..Default::default()
    };
    assert_eq!(cache.EstimateDataLength(&ordinary), (10, 7, 70, 60));

    let partitioned = TableMeta {
        ID: 20,
        Columns: columns.clone(),
        Indices: vec![
            local,
            IndexMeta {
                ID: 6,
                Public: true,
                Global: true,
                Columns: vec![IndexColumnMeta {
                    Offset: 0,
                    Length: None,
                }],
            },
        ],
        Partitions: vec![21, 22],
        ..Default::default()
    };
    assert_eq!(cache.EstimateDataLength(&partitioned), (8, 7, 56, 80));

    let sequence = TableMeta {
        ID: 30,
        Columns: columns,
        Indices: vec![],
        IsSequence: true,
        ..Default::default()
    };
    assert_eq!(cache.EstimateDataLength(&sequence), (1, 7, 84, 0));
}

/// 验证 `cacheOfBatchUpdate`：更新/删除缓冲满批时触发回调，`flush` 清空残余。
#[test]
fn TestCacheOfBatchUpdate() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let marked_as_updated = Rc::new(RefCell::new(Vec::new()));
    let marked_as_deleted = Rc::new(RefCell::new(Vec::new()));
    let test_batch_size = 3;
    let updated = Rc::clone(&marked_as_updated);
    let deleted = Rc::clone(&marked_as_deleted);
    let mut cached = newCacheOfBatchUpdate(test_batch_size, move |to_update, to_delete| {
        for table in to_update {
            updated.borrow_mut().push(table.PhysicalID);
        }
        deleted.borrow_mut().extend_from_slice(to_delete);
    });

    // 前 3 条仅入缓冲，第 4 条触发满批 flush。
    cached.addToUpdate(Arc::new(table(1, 0)));
    cached.addToUpdate(Arc::new(table(2, 0)));
    cached.addToUpdate(Arc::new(table(3, 0)));
    assert_eq!(marked_as_updated.borrow().len(), 0);
    assert_eq!(cached.toUpdate.len(), 3);
    cached.addToUpdate(Arc::new(table(4, 0)));
    assert_eq!(marked_as_updated.borrow().len(), 3);
    assert_eq!(marked_as_updated.borrow()[0], 1);
    assert_eq!(marked_as_updated.borrow()[1], 2);
    assert_eq!(marked_as_updated.borrow()[2], 3);
    assert_eq!(cached.toUpdate.len(), 1);
    assert_eq!(cached.toUpdate[0].PhysicalID, 4);

    // 删除侧同样满批才 flush；交叉写入更新也可能触发更新侧 flush。
    cached.addToDelete(5);
    cached.addToDelete(6);
    cached.addToDelete(7);
    assert_eq!(marked_as_deleted.borrow().len(), 0);
    assert_eq!(cached.toDelete.len(), 3);
    cached.addToUpdate(Arc::new(table(8, 0)));
    assert_eq!(cached.toUpdate.len(), 2);
    cached.addToDelete(9);
    assert_eq!(marked_as_deleted.borrow().len(), 3);
    assert_eq!(marked_as_deleted.borrow()[0], 5);
    assert_eq!(marked_as_deleted.borrow()[1], 6);
    assert_eq!(marked_as_deleted.borrow()[2], 7);
    assert_eq!(cached.toDelete.len(), 1);
    assert_eq!(cached.toDelete[0], 9);
    assert_eq!(marked_as_updated.borrow().len(), 5);
    assert_eq!(marked_as_updated.borrow()[3], 4);
    assert_eq!(marked_as_updated.borrow()[4], 8);

    cached.flush();
    assert_eq!(cached.toUpdate.len(), 0);
    assert_eq!(cached.toDelete.len(), 0);
    assert_eq!(marked_as_deleted.borrow().len(), 4);
    assert_eq!(marked_as_deleted.borrow()[3], 9);
    cached.flush();
    assert_eq!(marked_as_deleted.borrow().len(), 4);
    assert_eq!(marked_as_updated.borrow().len(), 5);
}

/// 验证 `UpdateStatsHealthyMetrics` 按伪统计、健康度区间与 unneeded analyze 分桶写仪表。
#[test]
fn TestUpdateStatsHealthyMetrics() {
    reset_healthy_gauges();
    let _guard = HealthyGaugeGuard;

    let fake_analyze_version = 3959837493728947298_u64;
    // 构造覆盖各健康度区间、伪统计与无需 ANALYZE 的样例表。
    let tables = [
        new_mock_table(0, false, 2000, 1000, 0), // never analyzed -> [0,50)
        new_mock_table(1, false, 2000, 1100, fake_analyze_version), // [0,50)
        new_mock_table(2, false, 2000, 920, fake_analyze_version), // [50,55)
        new_mock_table(3, false, 2000, 200, fake_analyze_version), // [80,100)
        new_mock_table(4, false, 2000, 0, fake_analyze_version), // [100,100]
        new_mock_table(5, true, 10000, 0, 0),    // pseudo
        new_mock_table(6, false, 800, 500, fake_analyze_version), // [0,50)
        new_mock_table(7, false, 800, 500, 0),   // unneeded analyze
    ];

    let cache_impl = NewStatsCacheImplForTest().unwrap();
    for table in tables {
        let id = table.PhysicalID;
        cache_impl.Put(id, Arc::new(table));
    }
    cache_impl.UpdateStatsHealthyMetrics();

    assert_eq!(StatsHealthyGauges.len(), StatsHealthyBucketCount);
    let expected = [
        ("[0,50)", 3.0),
        ("[50,55)", 1.0),
        ("[55,60)", 0.0),
        ("[60,70)", 0.0),
        ("[70,80)", 0.0),
        ("[80,100)", 1.0),
        ("[100,100]", 1.0),
        ("[0,100]", 8.0),
        ("unneeded analyze", 1.0),
        ("pseudo", 1.0),
    ];
    for (idx, gauge) in StatsHealthyGauges.iter().enumerate() {
        let cfg = HEALTHY_BUCKET_CONFIGS[idx];
        assert_eq!(expected[idx].0, cfg.label);
        assert_eq!(expected[idx].1, gauge.get());
    }
}

/// 验证健康度边界值落入与 Go 一致的桶下标。
#[test]
fn stats_healthy_bucket_index_matches_go_bounds() {
    assert_eq!(statsHealthyBucketIndex(0), 0);
    assert_eq!(statsHealthyBucketIndex(49), 0);
    assert_eq!(statsHealthyBucketIndex(50), 1);
    assert_eq!(statsHealthyBucketIndex(54), 1);
    for (healthy, index) in [
        (55, 2),
        (59, 2),
        (60, 3),
        (69, 3),
        (70, 4),
        (79, 4),
        (80, 5),
        (99, 5),
    ] {
        assert_eq!(statsHealthyBucketIndex(healthy), index);
    }
    assert_eq!(statsHealthyBucketIndex(90), 5);
    assert_eq!(statsHealthyBucketIndex(100), 6);
}

/// 构造测试用 `StatisticsTable`：指定伪标志、行数、修改量、ANALYZE 版本与健康度。
fn new_mock_table(
    physical_id: i64,
    pseudo: bool,
    realtime_count: i64,
    modify_count: i64,
    analyze_version: u64,
) -> StatisticsTable {
    let mut table = StatisticsTable::New(physical_id, realtime_count, modify_count);
    table.Pseudo = pseudo;
    table.LastAnalyzeVersion = analyze_version;
    table
}

/// 将全部健康度仪表归零，避免测试间互相污染。
fn reset_healthy_gauges() {
    for gauge in StatsHealthyGauges.iter() {
        gauge.set(0.0);
    }
}

/// 作用域结束时自动重置健康度仪表的 RAII 守卫。
struct HealthyGaugeGuard;

impl Drop for HealthyGaugeGuard {
    fn drop(&mut self) {
        reset_healthy_gauges();
    }
}

#[test]
fn quota_eviction_retains_table_metadata_like_go_lfu() {
    let cache = NewStatsCacheImplForTest().unwrap();
    let mut table = cache_testutil::NewMockStatisticsTable(1, 1, true, true, true);
    table.PhysicalID = 42;
    table.Version = 7;
    table.RealtimeCount = 2000;
    let before = table.MemoryUsage().TotalTrackingMemUsage();
    assert!(before > 1);
    cache.Put(42, Arc::new(table));
    cache.SetStatsCacheCapacity(1);
    cache.TriggerEvict();
    cache.WaitForAsyncUpdates();
    let table = cache.Get(42).expect("eviction must retain table metadata");
    assert_eq!(table.Version, 7);
    assert_eq!(table.RealtimeCount, 2000);
    assert_eq!(cache.Len(), 1);
    assert!(table.MemoryUsage().TotalTrackingMemUsage() < before);
}

fn table(id: i64, version: u64) -> StatisticsTable {
    let mut table = StatisticsTable::New(id, 0, 0);
    table.Version = version;
    table
}

fn cache_with_lease(millis: u64) -> crate::StatsCacheImpl {
    let handle = Arc::new(TestHandle::new(Vec::new()));
    handle
        .lease_ms
        .store(millis, std::sync::atomic::Ordering::Release);
    NewStatsCacheImpl(handle).unwrap()
}

use stats_util::{Row, SqlValue, StatsError};
use std::sync::{
    Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

struct Globals;
impl stats_util::GlobalVariableAccessor for Globals {
    fn get_global_sys_var(&self, name: &str) -> Result<String, StatsError> {
        Ok(if name == "time_zone" { "UTC" } else { "1" }.into())
    }
}

#[derive(Default)]
struct QueryState {
    rows: Vec<Row>,
    queries: Vec<(String, Vec<SqlValue>)>,
    error: Option<StatsError>,
}
struct TestSession {
    state: Arc<Mutex<QueryState>>,
    vars: Arc<stats_util::SessionVariables>,
    returned: AtomicUsize,
}
impl stats_util::SessionPool for TestSession {
    fn with_session(
        &self,
        callback: &mut dyn FnMut(&dyn stats_util::SessionContext) -> Result<(), StatsError>,
    ) -> Result<(), StatsError> {
        let result = callback(self);
        self.returned.fetch_add(1, Ordering::Relaxed);
        result
    }
}
impl stats_util::SessionContext for TestSession {
    fn session_variables(&self) -> Arc<stats_util::SessionVariables> {
        self.vars.clone()
    }
    fn transaction(&self, _: bool) -> Result<Arc<dyn stats_util::Transaction>, StatsError> {
        unreachable!("refresh does not open transactions")
    }
    fn sql_executor(&self) -> Arc<dyn stats_util::SqlExecutor> {
        unreachable!("refresh uses restricted SQL")
    }
    fn restricted_sql_executor(&self) -> Arc<dyn stats_util::RestrictedSqlExecutor> {
        Arc::new(QueryExecutor(self.state.clone()))
    }
    fn set_system_variable(&self, _: &str, _: &str) -> Result<(), StatsError> {
        Ok(())
    }
    fn location(&self) -> String {
        "UTC".into()
    }
}
struct QueryExecutor(Arc<Mutex<QueryState>>);
impl stats_util::RestrictedSqlExecutor for QueryExecutor {
    fn exec_restricted_sql(
        &self,
        _: &stats_util::ExecutionContext,
        options: &[stats_util::ExecOption],
        sql: &str,
        args: &[SqlValue],
    ) -> Result<(Vec<Row>, Vec<stats_util::ResultField>), StatsError> {
        assert_eq!(options, stats_util::USE_CURRENT_SESSION_OPTIONS);
        let mut state = self.0.lock().unwrap();
        state.queries.push((sql.into(), args.to_vec()));
        if let Some(error) = state.error.clone() {
            return Err(error);
        }
        Ok((state.rows.clone(), vec![]))
    }
}
struct TestHandle {
    lease_ms: AtomicU64,
    session: Arc<TestSession>,
    loaded: Mutex<Vec<i64>>,
    missing: Mutex<Vec<i64>>,
    load_results: Mutex<HashMap<i64, stats_types::Result<Option<Arc<StatisticsTable>>>>>,
    cancel_after: Mutex<Option<(usize, stats_types::ExecutionContext)>>,
}
impl TestHandle {
    fn new(rows: Vec<Row>) -> Self {
        Self {
            lease_ms: AtomicU64::new(0),
            session: Arc::new(TestSession {
                state: Arc::new(Mutex::new(QueryState {
                    rows,
                    ..Default::default()
                })),
                vars: Arc::new(stats_util::SessionVariables::new(Arc::new(Globals))),
                returned: AtomicUsize::new(0),
            }),
            loaded: Mutex::new(vec![]),
            missing: Mutex::new(vec![]),
            load_results: Mutex::new(HashMap::new()),
            cancel_after: Mutex::new(None),
        }
    }
}
impl crate::StatsCacheHandle for TestHandle {
    fn lease(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.lease_ms.load(Ordering::Acquire))
    }
    fn session_pool(&self) -> Arc<dyn stats_util::SessionPool> {
        self.session.clone()
    }
    fn table_info_by_id(
        &self,
        _: &dyn stats_types::InfoSchema,
        id: i64,
    ) -> stats_types::Result<Option<Arc<model::TableInfo>>> {
        Ok((!self.missing.lock().unwrap().contains(&id)).then(|| {
            Arc::new(model::TableInfo {
                ID: id,
                UpdateTS: 99,
                ..Default::default()
            })
        }))
    }
    fn table_stats_from_storage(
        &self,
        info: &model::TableInfo,
        id: i64,
        load_all: bool,
        snapshot: u64,
    ) -> stats_types::Result<Option<Arc<StatisticsTable>>> {
        assert_eq!(info.ID, id);
        assert!(!load_all);
        assert_eq!(snapshot, 0);
        let mut loaded = self.loaded.lock().unwrap();
        loaded.push(id);
        if let Some((after, context)) = self.cancel_after.lock().unwrap().as_ref() {
            if loaded.len() == *after {
                context.cancel();
            }
        }
        self.load_results
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_else(|| Ok(Some(Arc::new(table(id, 0)))))
    }
}
fn meta(id: i64, version: u64, hist: Option<u64>) -> Row {
    Row {
        values: vec![
            SqlValue::Unsigned(version),
            SqlValue::Integer(id),
            SqlValue::Integer(8),
            SqlValue::Integer(100),
            SqlValue::Unsigned(123),
            hist.map(SqlValue::Unsigned).unwrap_or(SqlValue::Null),
        ],
    }
}
fn refresh_cache(handle: Arc<TestHandle>, quota: bool) -> crate::StatsCacheImpl {
    crate::StatsCacheImpl::new(Some(handle), Some((quota, 5_000_000))).unwrap()
}

#[test]
fn go_merge_47_selects_stats_meta_index_for_refresh_mode() {
    for (ids, hint) in [(Vec::new(), "idx_ver"), (vec![7], "tbl")] {
        let handle = Arc::new(TestHandle::new(Vec::new()));
        let cache = refresh_cache(handle.clone(), false);
        cache
            .Update(&Default::default(), &infoschema::infoSchema::new(1), &ids)
            .unwrap();
        let query = &handle.session.state.lock().unwrap().queries[0].0;
        assert!(
            query.starts_with(&format!(
                "SELECT /*+ use_index(mysql.stats_meta, {hint}) */ version"
            )),
            "unexpected query: {query}"
        );
    }
}

#[test]
fn refresh_sorts_ids_and_preserves_go_version_rules_in_both_backends() {
    for quota in [true, false] {
        let handle = Arc::new(TestHandle::new(vec![meta(2, 8, Some(5)), meta(3, 9, None)]));
        let cache = refresh_cache(handle.clone(), quota);
        cache.Put(1, Arc::new(table(1, 4)));
        let ids = [3, 2, 3];
        cache
            .Update(&Default::default(), &infoschema::infoSchema::new(1), &ids)
            .unwrap();
        assert_eq!(ids, [3, 2, 3]);
        let state = handle.session.state.lock().unwrap();
        assert_eq!(state.queries[0], (
            "SELECT /*+ use_index(mysql.stats_meta, tbl) */ version, table_id, modify_count, count, snapshot, last_stats_histograms_version from mysql.stats_meta where version > %? and table_id in (%?) order by version".into(),
            vec![SqlValue::Unsigned(4), SqlValue::StringList(vec!["2".into(), "3".into()])]
        ));
        assert_eq!(handle.session.returned.load(Ordering::Relaxed), 1);
        assert_eq!(cache.MaxTableStatsVersion(), if quota { 4 } else { 9 });
        let updated = cache.Get(2).unwrap();
        assert_eq!(
            (
                updated.Version,
                updated.LastStatsHistVersion,
                updated.LastAnalyzeVersion,
                updated.TblInfoUpdateTS
            ),
            (8, 5, 123, 99)
        );
        assert_eq!((updated.RealtimeCount, updated.ModifyCount), (100, 8));
        assert_eq!(cache.Get(3).unwrap().LastStatsHistVersion, 0);
    }
}

#[test]
fn refresh_reuses_histograms_skips_old_rows_and_handles_missing_and_failed_loads() {
    let handle = Arc::new(TestHandle::new(
        (1..=6).map(|id| meta(id, 10, Some(5))).collect(),
    ));
    handle.missing.lock().unwrap().push(1);
    handle.load_results.lock().unwrap().insert(2, Ok(None));
    handle
        .load_results
        .lock()
        .unwrap()
        .insert(3, Err(stats_types::Error("DDL changed".into())));
    let cache = refresh_cache(handle.clone(), true);
    for id in 1..=6 {
        let mut t = cache_testutil::NewMockStatisticsTable(1, 1, true, true, true);
        t.PhysicalID = id;
        t.Version = if id == 4 || id == 6 { 11 } else { 1 };
        t.TblInfoUpdateTS = if id == 6 { 98 } else { 99 };
        t.LastStatsHistVersion = if id == 5 { 5 } else { 0 };
        t.LastAnalyzeVersion = if id == 5 { 42 } else { 0 };
        cache.Put(id, Arc::new(t));
    }
    let old = cache.Get(5).unwrap();
    cache
        .Update(&Default::default(), &infoschema::infoSchema::new(1), &[])
        .unwrap();
    assert!(cache.Get(1).is_none());
    assert!(cache.Get(2).is_none());
    assert_eq!(cache.Get(3).unwrap().Version, 1);
    assert_eq!(cache.Get(4).unwrap().Version, 11);
    let reused = cache.Get(5).unwrap();
    assert_eq!(reused.Version, 10);
    assert_eq!(reused.LastAnalyzeVersion, 42);
    assert_eq!(old.Version, 1); // CopyAs must not mutate an already published snapshot.
    assert_eq!(
        old.MemoryUsage().TotalMemUsage,
        reused.MemoryUsage().TotalMemUsage
    );
    assert_eq!(*handle.loaded.lock().unwrap(), vec![2, 3, 6]);
    assert_eq!(cache.MaxTableStatsVersion(), 11);
    assert!(
        handle.session.state.lock().unwrap().queries[0]
            .0
            .ends_with("where version > %? order by version")
    );
}

#[test]
fn refresh_errors_cancel_without_flushing_pending_batch() {
    let handle = Arc::new(TestHandle::new(
        (1..=13).map(|id| meta(id, id as u64, None)).collect(),
    ));
    let context = stats_types::ExecutionContext::default();
    *handle.cancel_after.lock().unwrap() = Some((12, context.clone()));
    let cache = refresh_cache(handle.clone(), true);
    assert_eq!(
        cache
            .Update(&context, &infoschema::infoSchema::new(1), &[])
            .unwrap_err()
            .0,
        "context canceled"
    );
    assert_eq!(cache.Len(), 10);
    assert_eq!(cache.MaxTableStatsVersion(), 10);
    assert!(cache.Get(11).is_none());
    assert!(cache.Get(12).is_none());
    handle.session.state.lock().unwrap().error = Some(StatsError::Sql("query failed".into()));
    let error = cache
        .Update(&Default::default(), &infoschema::infoSchema::new(1), &[])
        .unwrap_err();
    assert!(error.0.contains("query failed"));
    assert_eq!(cache.Len(), 10);
    assert_eq!(handle.session.returned.load(Ordering::Relaxed), 2);
}

#[test]
fn replace_shares_cache_and_map_copy_preserves_prior_snapshot() {
    let first = crate::StatsCacheImpl::new(None, Some((false, 0))).unwrap();
    first.Put(1, Arc::new(table(1, 1)));
    let second = crate::StatsCacheImpl::new(None, Some((false, 0))).unwrap();
    stats_types::StatsCache::Replace(&second, &first);
    second.Put(2, Arc::new(table(2, 2)));
    assert!(first.Get(2).is_some());
    second.UpdateStatsCache(&[Arc::new(table(3, 3))], &[1], true);
    assert!(first.Get(1).is_some());
    assert!(first.Get(3).is_none());
    assert!(second.Get(1).is_none());
    assert_eq!(second.MaxTableStatsVersion(), 3);
    second.Clear();
    assert_eq!(second.Len(), 0);
    assert_eq!(second.MaxTableStatsVersion(), 0);
    assert_eq!(first.Len(), 2);
}

#[test]
fn lease_is_read_dynamically_and_tso_underflow_is_zero() {
    let handle = Arc::new(TestHandle::new(vec![]));
    let cache = refresh_cache(handle.clone(), true);
    cache.Put(1, Arc::new(table(1, 10_000 << 18)));
    handle.lease_ms.store(1_000, Ordering::Release);
    assert_eq!(cache.GetNextCheckVersionWithOffset(), 5_000 << 18);
    handle.lease_ms.store(3_000, Ordering::Release);
    assert_eq!(cache.GetNextCheckVersionWithOffset(), 0);
    assert!(crate::NewStatsCacheWithCapacity(true, -1).is_err());
}

/// Configuration and Prometheus handles are process-wide in Go; isolate mutations
/// in a child test process so ordinary cache tests can still run concurrently.
#[test]
#[allow(static_mut_refs)]
fn global_configuration_metrics_and_failpoints() {
    const CHILD: &str = "ASTERSQL_1302_CACHE_GLOBAL_TEST";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "statscache_test::global_configuration_metrics_and_failpoints",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    cache_metrics::metrics::init_parent_metrics();
    cache_metrics::cache_metrics::InitMetricsVars();
    unsafe {
        metrics::stats::InitStatsMetrics();
    }
    config::update_global(|c| c.performance.enable_stats_cache_mem_quota = false);
    let handle = Arc::new(TestHandle::new(vec![]));
    let cache = NewStatsCacheImpl(handle.clone()).unwrap();
    let mut original = cache_testutil::NewMockStatisticsTable(1, 0, true, false, false);
    original.PhysicalID = 1;
    original.Version = 1;
    cache.Put(1, Arc::new(original));
    assert!(cache.Get(1).is_some());
    assert!(cache.Get(2).is_none());
    cache.UpdateStatsCache(&[Arc::new(table(2, 2))], &[], true);
    assert_eq!(cache.MaxTableStatsVersion(), 2);
    unsafe {
        assert_eq!(
            cache_metrics::cache_metrics::CostGauge
                .as_ref()
                .unwrap()
                .get(),
            cache.MemConsumed() as f64
        );
        assert_eq!(
            cache_metrics::cache_metrics::HitCounter
                .as_ref()
                .unwrap()
                .get(),
            1.0
        );
        assert_eq!(
            cache_metrics::cache_metrics::MissCounter
                .as_ref()
                .unwrap()
                .get(),
            1.0
        );
        assert_eq!(
            cache_metrics::cache_metrics::UpdateCounter
                .as_ref()
                .unwrap()
                .get(),
            1.0
        );
    }
    const POINT: &str = "github.com/pingcap/tidb/pkg/statistics/handle/cache/StatsCacheGetNil";
    {
        let _guard = testfailpoint::enable(POINT, "return()");
        assert!(cache.Get(1).is_none());
    }
    {
        let _guard = testfailpoint::enable(POINT, "off");
        assert!(cache.Get(1).is_some());
    }
    config::update_global(|c| c.performance.enable_stats_cache_mem_quota = true);
    vardef::StatsCacheMemQuota.Store(-1);
    assert!(NewStatsCacheImpl(handle.clone()).is_err());
    assert!(NewStatsCacheImplForTest().is_err());
    cache.Clear(); // A failed new cache must preserve the current one.
    assert_eq!(cache.Len(), 2);
    cache
        .Update(&Default::default(), &infoschema::infoSchema::new(0), &[])
        .unwrap();
    handle.session.state.lock().unwrap().error = Some(StatsError::Sql("query failed".into()));
    assert!(
        cache
            .Update(&Default::default(), &infoschema::infoSchema::new(0), &[])
            .is_err()
    );
    unsafe {
        assert_eq!(
            metrics::stats::StatsDeltaLoadHistogram
                .as_ref()
                .unwrap()
                .get_sample_count(),
            2
        );
    }
    vardef::StatsCacheMemQuota.Store(5_000_000);
    cache.Clear();
    assert_eq!(cache.Len(), 0);
    cache.UpdateStatsCache(&[Arc::new(table(3, 3))], &[3], false);
    unsafe {
        assert_eq!(
            cache_metrics::cache_metrics::DelCounter
                .as_ref()
                .unwrap()
                .get(),
            1.0
        );
    }
    cache.Close();
    cache.Close();
}

#[test]
fn refresh_uses_real_schema_metadata_and_partitions() {
    struct Handle(Arc<TestHandle>);
    impl crate::StatsCacheHandle for Handle {
        fn lease(&self) -> std::time::Duration {
            std::time::Duration::ZERO
        }
        fn session_pool(&self) -> Arc<dyn stats_util::SessionPool> {
            self.0.session.clone()
        }
        fn table_stats_from_storage(
            &self,
            info: &model::TableInfo,
            id: i64,
            all: bool,
            snapshot: u64,
        ) -> stats_types::Result<Option<Arc<StatisticsTable>>> {
            assert_eq!(info.ID, 1);
            assert_eq!(info.UpdateTS, 456);
            assert!(!all);
            assert_eq!(snapshot, 0);
            assert!([1, 11].contains(&id));
            Ok(Some(Arc::new(table(id, 0))))
        }
    }
    let mut schema = infoschema::infoSchema::new(1);
    schema.add_schema(
        infoschema::DBInfo {
            id: 1,
            name: infoschema::CiString::new("test"),
            ..Default::default()
        },
        vec![infoschema::Table::from_model(model::TableInfo {
            ID: 1,
            UpdateTS: 456,
            Partition: Some(model::PartitionInfo {
                Definitions: vec![model::PartitionDefinition {
                    ID: 11,
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        })],
    );
    let h = Arc::new(TestHandle::new(vec![
        meta(1, 1, None),
        meta(11, 2, None),
        meta(12, 3, None),
    ]));
    let cache = NewStatsCacheImpl(Arc::new(Handle(h))).unwrap();
    cache.Put(12, Arc::new(table(12, 0)));
    cache.Update(&Default::default(), &schema, &[]).unwrap();
    assert!(cache.Get(12).is_none());
    assert_eq!(cache.Get(1).unwrap().TblInfoUpdateTS, 456);
    assert_eq!(cache.Get(11).unwrap().TblInfoUpdateTS, 456);
    assert_eq!(cache.MaxTableStatsVersion(), 2);
}

#[test]
fn constructor_accepts_existing_dynamic_stats_handle_contract() {
    let _constructor: fn(
        Arc<dyn stats_types::StatsHandle>,
    ) -> Result<crate::StatsCacheImpl, crate::CacheError> =
        NewStatsCacheImpl::<dyn stats_types::StatsHandle>;
}
