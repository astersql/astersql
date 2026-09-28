// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// MockStore + Domain 上的统计信息（stats）集成测试。
//
// 覆盖 ANALYZE/FLUSH、会话隔离、stats lock/unlock 事务性、
// 批量 flush 失败重试、GC、历史保留与受限查询投影一致性等。

use crate::mockstore::CreateMockStoreAndDomain;
use crate::{Database, DbValue, TestKit};
use astersql_domain::domain::{DomainStatsBackend, Handle, KvStatsStore, StatsKvStorage};
use astersql_sessionctx_vardef as vardef;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// RAII：临时设置统计缓存内存配额并在 Drop 时恢复。
struct StatsCacheQuotaGuard {
    _lock: MutexGuard<'static, ()>,
    restore_config: Option<Box<dyn FnOnce()>>,
    previous_quota: i64,
}

impl StatsCacheQuotaGuard {
    /// 加进程锁、开启配额配置并写入目标配额。
    fn set(quota: i64) -> Self {
        static CONFIG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = CONFIG_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("statistics cache quota lock poisoned");
        let restore_config = astersql_config::restore_func();
        let previous_quota = vardef::StatsCacheMemQuota.Load();
        astersql_config::update_global(|config| {
            config.performance.enable_stats_cache_mem_quota = true;
        });
        vardef::StatsCacheMemQuota.Store(quota);
        Self {
            _lock: lock,
            restore_config: Some(Box::new(restore_config)),
            previous_quota,
        }
    }
}

/// 恢复配额与全局配置。
impl Drop for StatsCacheQuotaGuard {
    fn drop(&mut self) {
        vardef::StatsCacheMemQuota.Store(self.previous_quota);
        if let Some(restore) = self.restore_config.take() {
            restore();
        }
    }
}

/// ANALYZE + FLUSH 后 Domain handle 暴露真实物理统计。
#[test]
fn mock_store_and_domain_expose_real_analyzed_and_flushed_physical_stats() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);

    testkit.MustExec("create table t(a int)", Vec::new());
    testkit.MustExec(
        "insert into t values (?), (?), (?)",
        vec![DbValue::I64(1), DbValue::I64(2), DbValue::I64(3)],
    );
    testkit.MustExec("analyze table t", Vec::new());
    testkit.MustExec("insert into t values (4)", Vec::new());
    testkit.MustExec("flush stats_delta t", Vec::new());

    let handle = domain.stats_handle();
    let handle = handle.lock().expect("domain statistics handle");
    let physical_stats = handle.stats_meta_rows();
    assert_eq!(physical_stats.len(), 1);
    assert_eq!(physical_stats[0].realtime_count, 4);
    assert_eq!(physical_stats[0].modify_count, 1);
    assert!(!physical_stats[0].pseudo);
    assert_eq!(physical_stats[0].columns.len(), 1);
}

/// 统计缓存容量遵循作用域内全局配额。
#[test]
fn domain_stats_cache_capacity_uses_scoped_global_quota() {
    let _config = StatsCacheQuotaGuard::set(4096);
    let (_store, domain) = CreateMockStoreAndDomain();
    assert_eq!(
        domain
            .stats_handle()
            .lock()
            .expect("statistics handle")
            .cache()
            .capacity(),
        4096
    );
}

/// 同一 Domain 上多会话独立（含 snapshot 与 SQLKiller）。
#[test]
fn mock_store_creates_independent_sessions_over_one_domain() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut first = TestKit::new(store.clone());
    let first_killer = store.sql_killer();
    let mut second = TestKit::new(store.clone());
    let second_killer = store.sql_killer();

    assert!(!Arc::ptr_eq(&first_killer, &second_killer));

    first.MustExec("create table shared_t(a int)", Vec::new());
    first.MustExec("insert into shared_t values (1), (2)", Vec::new());
    let schema_version = domain.stats_context().catalog_version();
    first.MustExec(
        &format!("set @@tidb_snapshot = {schema_version}"),
        Vec::new(),
    );

    second.MustExec("create table second_t(a int)", Vec::new());
    second.MustExec("analyze table shared_t", Vec::new());

    let first_tables = first.MustQuery("show table status", Vec::new()).Rows();
    assert!(first_tables.iter().flatten().any(|name| name == "shared_t"));
    assert!(!first_tables.iter().flatten().any(|name| name == "second_t"));
    let second_tables = second.MustQuery("show table status", Vec::new()).Rows();
    assert!(
        second_tables
            .iter()
            .flatten()
            .any(|name| name == "shared_t")
    );
    assert!(
        second_tables
            .iter()
            .flatten()
            .any(|name| name == "second_t")
    );

    let handle = domain.stats_handle();
    let handle = handle.lock().expect("domain statistics handle");
    assert!(
        handle
            .stats_meta_rows()
            .iter()
            .any(|stats| !stats.pseudo && stats.realtime_count == 2)
    );
}

/// 会话显式 close / Drop 会 join worker 并清零活跃计数。
#[test]
fn analyze_session_workers_join_on_explicit_close_and_drop() {
    let (store, _) = CreateMockStoreAndDomain();
    assert_eq!(store.active_session_count(), 0);

    let first = TestKit::new(store.clone());
    assert_eq!(store.active_session_count(), 1);
    let first_session = first.Session();
    first_session.close().expect("close first session");
    first_session.close().expect("close first session again");
    assert_eq!(store.active_session_count(), 0);
    drop(first_session);
    drop(first);

    {
        let _second = TestKit::new(store.clone());
        assert_eq!(store.active_session_count(), 1);
    }
    assert_eq!(store.active_session_count(), 0);

    let third = TestKit::new(store.clone());
    assert_eq!(store.active_session_count(), 1);
    store.close().expect("close analyze store");
    store.close().expect("close analyze store again");
    assert_eq!(store.active_session_count(), 0);
    assert_eq!(
        third.QueryToErr("show table status").message(),
        "canonical analyze session is closed"
    );
}

/// 受限统计查询结果与普通会话查询共享同一行集。
#[test]
fn restricted_stats_rows_are_shared_by_normal_sessions() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut writer = TestKit::new(store.clone());
    let reader = TestKit::new(store);
    writer.MustExec("create table shared_stats(a int)", Vec::new());
    writer.MustExec("analyze table shared_stats", Vec::new());
    writer.MustExec("lock stats shared_stats", Vec::new());

    let table_id = domain
        .stats_table("test", "shared_stats")
        .expect("shared_stats metadata")
        .0
        .table_id;
    assert_eq!(
        reader
            .MustQuery(
                &format!(
                    "select table_id, count, modify_count from mysql.stats_table_locked where table_id = {table_id}"
                ),
                Vec::new(),
            )
            .Rows(),
        domain
            .restricted_stats_query(
                &format!(
                    "select table_id, count, modify_count from mysql.stats_table_locked where table_id = {table_id}"
                ),
                &[],
            )
            .expect("restricted statistics query")
    );
}

/// unlock 失败时 meta 与 locked 行一并回滚。
#[test]
fn unlock_failure_rolls_back_meta_and_locked_row_together() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table rollback_stats(a int)", Vec::new());
    testkit.MustExec("analyze table rollback_stats", Vec::new());
    testkit.MustExec("lock stats rollback_stats", Vec::new());
    testkit.MustExec("insert into rollback_stats values (1), (2)", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());

    let table_id = domain
        .stats_table("test", "rollback_stats")
        .expect("rollback_stats metadata")
        .0
        .table_id;
    let meta_sql = format!(
        "select table_id, count, modify_count from mysql.stats_meta where table_id = {table_id}"
    );
    let lock_sql = format!(
        "select table_id, count, modify_count from mysql.stats_table_locked where table_id = {table_id}"
    );
    let meta_before = testkit.MustQuery(&meta_sql, Vec::new()).Rows();
    let lock_before = testkit.MustQuery(&lock_sql, Vec::new()).Rows();

    // 注入 lock 删除失败，断言 meta/locked 行同时回滚。
    domain.fail_next_stats_lock_delete_for_test();
    assert!(
        testkit
            .QueryToErr("unlock stats rollback_stats")
            .message()
            .ends_with("injected statistics lock delete failure")
    );
    assert_eq!(testkit.MustQuery(&meta_sql, Vec::new()).Rows(), meta_before);
    assert_eq!(testkit.MustQuery(&lock_sql, Vec::new()).Rows(), lock_before);
}

/// unlock 提交后 KV 与 handle 一致，后续 persist 不漂移。
#[test]
fn unlock_commit_keeps_kv_and_handle_equal_across_later_persist() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table unlock_cache(a int)", Vec::new());
    testkit.MustExec("insert into unlock_cache values (1)", Vec::new());
    testkit.MustExec("analyze table unlock_cache", Vec::new());
    testkit.MustExec("lock stats unlock_cache", Vec::new());
    testkit.MustExec("insert into unlock_cache values (2), (3)", Vec::new());
    testkit.MustExec("flush stats_delta unlock_cache", Vec::new());
    testkit.MustExec("unlock stats unlock_cache", Vec::new());

    let table_id = domain
        .stats_table("test", "unlock_cache")
        .expect("unlock_cache metadata")
        .0
        .table_id;
    let query = format!(
        "select version, count, modify_count from mysql.stats_meta where table_id={table_id}"
    );
    let kv_after_unlock = testkit.MustQuery(&query, Vec::new()).Rows();
    let handle_after_unlock = domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .stats_meta(table_id)
        .cloned()
        .expect("cached table statistics");
    assert_eq!(
        kv_after_unlock,
        vec![vec![
            handle_after_unlock.version.to_string(),
            handle_after_unlock.realtime_count.to_string(),
            handle_after_unlock.modify_count.to_string(),
        ]]
    );
    assert_eq!(
        (
            handle_after_unlock.realtime_count,
            handle_after_unlock.modify_count
        ),
        (3, 2)
    );

    domain
        .persist_stats_meta(&[table_id])
        .expect("persist synchronized statistics");
    assert_eq!(
        testkit.MustQuery(&query, Vec::new()).Rows(),
        kv_after_unlock
    );
}

/// 受限查询保留文本绑定、别名与限定列投影。
#[test]
fn restricted_query_preserves_text_bindings_aliases_and_qualified_columns() {
    let (_store, domain) = CreateMockStoreAndDomain();
    domain.gc_stats(Duration::ZERO).expect("write GC timestamp");

    let rows = domain
        .restricted_stats_query(
            "SELECT t.variable_value AS gc_value FROM mysql.tidb AS t \
             WHERE t.variable_name = %? ORDER BY t.variable_value",
            &[astersql_domain::domain::SqlValue::Text(
                "tidb_stats_gc_last_ts".to_owned(),
            )],
        )
        .expect("typed text binding and aliased qualified projection");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 1);
    assert!(rows[0][0].parse::<u64>().is_ok());
}

/// 多表 flush 失败可重试且无丢失/双写。
#[test]
fn failed_multi_table_stats_flush_retries_without_loss_or_double_apply() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table flush_a(a int)", Vec::new());
    testkit.MustExec("create table flush_b(a int)", Vec::new());
    testkit.MustExec("analyze table flush_a, flush_b", Vec::new());
    testkit.MustExec("lock stats flush_a", Vec::new());
    testkit.MustExec("insert into flush_a values (1)", Vec::new());
    testkit.MustExec("insert into flush_b values (1)", Vec::new());
    let first = domain
        .stats_table("test", "flush_a")
        .expect("flush_a metadata")
        .0
        .table_id;
    let second = domain
        .stats_table("test", "flush_b")
        .expect("flush_b metadata")
        .0
        .table_id;

    // 首批 flush 注入失败，验证 pending delta 保留并可重试。
    domain.fail_stats_batch_after_for_test(1);
    assert!(
        testkit
            .QueryToErr("flush stats_delta flush_a, flush_b")
            .message()
            .contains("injected statistics batch persistence failure")
    );
    for table_id in [first, second] {
        testkit
            .MustQuery(
                &format!(
                    "select count,modify_count from mysql.stats_meta where table_id={table_id}"
                ),
                Vec::new(),
            )
            .Check(vec![vec!["0", "0"]]);
    }
    testkit
        .MustQuery(
            &format!(
                "select count,modify_count from mysql.stats_table_locked where table_id={first}"
            ),
            Vec::new(),
        )
        .Check(vec![vec!["0", "0"]]);
    assert_eq!(domain.stats_context().pending_stats_delta_ids().len(), 2);

    testkit.MustExec("flush stats_delta flush_a, flush_b", Vec::new());
    assert!(domain.stats_context().pending_stats_delta_ids().is_empty());
    testkit
        .MustQuery(
            &format!(
                "select count,modify_count from mysql.stats_table_locked where table_id={first}"
            ),
            Vec::new(),
        )
        .Check(vec![vec!["1", "1"]]);
    testkit
        .MustQuery(
            &format!("select count,modify_count from mysql.stats_meta where table_id={second}"),
            Vec::new(),
        )
        .Check(vec![vec!["1", "1"]]);
    let cached_second = domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .stats_meta(second)
        .cloned()
        .expect("cached table statistics");
    assert_eq!(
        (cached_second.realtime_count, cached_second.modify_count),
        (1, 1)
    );

    testkit.MustExec("unlock stats flush_a", Vec::new());
    testkit
        .MustQuery(
            &format!("select count,modify_count from mysql.stats_meta where table_id={first}"),
            Vec::new(),
        )
        .Check(vec![vec!["1", "1"]]);
}

/// 事务 commit 先于可重试的 stats outbox flush 成功。
#[test]
fn commit_succeeds_before_retryable_stats_outbox_flush() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table commit_stats(a int)", Vec::new());
    let table_key = domain
        .stats_table("test", "commit_stats")
        .expect("commit_stats metadata")
        .0;
    let table_id = table_key.table_id;
    testkit.MustExec("begin", Vec::new());
    testkit.MustExec("insert into commit_stats values (7)", Vec::new());
    domain.fail_next_stats_record_for_test();
    domain.fail_stats_batch_after_for_test(1);
    testkit.MustExec("commit", Vec::new());
    assert!(
        domain
            .record_stats_mutation(&table_key, 0, 0)
            .expect_err("commit must not consume the failable synchronous recorder")
            .to_string()
            .contains("injected statistics recording failure")
    );
    assert_eq!(
        testkit
            .Exec("update commit_stats set a=8 where a=7", Vec::new())
            .expect("committed row remains visible to a following DML")
            .affected_rows,
        1
    );
    assert_eq!(
        domain.stats_context().pending_stats_delta_ids(),
        vec![table_id]
    );

    assert!(
        testkit
            .QueryToErr("flush stats_delta commit_stats")
            .message()
            .contains("injected statistics batch persistence failure")
    );
    testkit.MustExec("flush stats_delta commit_stats", Vec::new());
    testkit
        .MustQuery(
            &format!("select count,modify_count from mysql.stats_meta where table_id={table_id}"),
            Vec::new(),
        )
        .Check(vec![vec!["1", "2"]]);
}

/// 删表后 GC 清除直方图与历史及 meta。
#[test]
fn stats_gc_removes_kv_histograms_and_history_with_dropped_meta() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table gc_stats(a int, index ia(a))", Vec::new());
    testkit.MustExec("analyze table gc_stats", Vec::new());
    let table_id = domain
        .stats_table("test", "gc_stats")
        .expect("gc_stats metadata")
        .0
        .table_id;
    domain
        .restricted_stats_execute(
            &format!(
                "insert into mysql.stats_meta_history (table_id, version, create_time) values ({table_id}, 1, 0)"
            ),
            &[],
        )
        .expect("insert meta history");
    domain
        .restricted_stats_execute(
            &format!(
                "insert into mysql.stats_history (table_id, version, seq_no, create_time) values ({table_id}, 1, 0, 0)"
            ),
            &[],
        )
        .expect("insert stats history");

    assert!(!testkit
        .MustQuery(
            &format!("select is_index, hist_id from mysql.stats_histograms where table_id = {table_id}"),
            Vec::new(),
        )
        .Rows()
        .is_empty());
    testkit.MustExec("drop table gc_stats", Vec::new());
    domain
        .gc_stats(Duration::ZERO)
        .expect("GC dropped histogram statistics");
    domain
        .gc_stats(Duration::ZERO)
        .expect("GC dropped meta statistics");

    for table in [
        "stats_meta",
        "stats_histograms",
        "stats_meta_history",
        "stats_history",
    ] {
        assert_eq!(
            testkit
                .MustQuery(
                    &format!("select count(*) from mysql.{table} where table_id = {table_id}"),
                    Vec::new(),
                )
                .Rows(),
            vec![vec!["0".to_owned()]],
            "{table} rows survived dropped-table GC"
        );
    }
}

/// 选择性 GC 只删除被 DROP 的直方图身份。
#[test]
fn stats_gc_deletes_only_the_dropped_histogram_identity() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table selective_gc(a int, b int, index ia(a), index ib(b))",
        Vec::new(),
    );
    testkit.MustExec("analyze table selective_gc", Vec::new());
    let (_, table) = domain
        .stats_table("test", "selective_gc")
        .expect("selective_gc metadata");
    let table_id = table.ID;
    let dropped_index = table
        .Indices
        .iter()
        .find(|index| index.Name.L == "ia")
        .expect("ia metadata")
        .ID;
    let retained_index = table
        .Indices
        .iter()
        .find(|index| index.Name.L == "ib")
        .expect("ib metadata")
        .ID;

    testkit.MustExec("alter table selective_gc drop index ia", Vec::new());
    domain
        .gc_stats(Duration::ZERO)
        .expect("selective histogram GC");

    testkit
        .MustQuery(
            &format!(
                "select hist_id from mysql.stats_histograms where table_id={table_id} and is_index=1 order by hist_id"
            ),
            Vec::new(),
        )
        .Check(vec![vec![retained_index.to_string()]]);
    assert_ne!(dropped_index, retained_index);
}

/// 历史 GC 保留新于保留期截止点的行。
#[test]
fn history_gc_retains_rows_newer_than_the_retention_cutoff() {
    let (_store, domain) = CreateMockStoreAndDomain();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("current time")
        .as_secs();
    let retained = now + 3_600;
    for table in ["stats_meta_history", "stats_history"] {
        let columns = if table == "stats_meta_history" {
            "table_id, version, create_time"
        } else {
            "table_id, version, seq_no, create_time"
        };
        let expired_values = if table == "stats_meta_history" {
            "(11, 1, 0)"
        } else {
            "(11, 1, 0, 0)"
        };
        let retained_values = if table == "stats_meta_history" {
            format!("(12, 2, {retained})")
        } else {
            format!("(12, 2, 0, {retained})")
        };
        domain
            .restricted_stats_execute(
                &format!(
                    "insert into mysql.{table} ({columns}) values {expired_values}, {retained_values}"
                ),
                &[],
            )
            .expect("insert history fixtures");
    }

    domain
        .clear_outdated_history_stats(Duration::from_secs(60))
        .expect("retention GC");

    for table in ["stats_meta_history", "stats_history"] {
        assert_eq!(
            domain
                .restricted_stats_query(
                    &format!(
                        "select table_id from mysql.{table} where create_time > {now} order by table_id"
                    ),
                    &[],
                )
                .expect("retained history query"),
            vec![vec!["12".to_owned()]]
        );
    }
}

/// GC 时间戳在 Store 重建后仍可从 KV 读回。
#[test]
fn stats_gc_timestamp_survives_store_recreation() {
    let (_store, domain) = CreateMockStoreAndDomain();
    domain.gc_stats(Duration::ZERO).expect("write GC timestamp");
    let sql = "select variable_value from mysql.tidb where variable_name='tidb_stats_gc_last_ts'";
    let written = domain
        .restricted_stats_query(sql, &[])
        .expect("read written GC timestamp");
    assert_eq!(written.len(), 1);

    let handle = Arc::new(Mutex::new(
        Handle::new(DomainStatsBackend::default(), false, false)
            .expect("construct restarted statistics handle"),
    ));
    let storage = domain.storage() as Arc<dyn StatsKvStorage>;
    let restarted = KvStatsStore::new(storage, handle);
    assert_eq!(
        restarted
            .query_strings(sql, &[])
            .expect("read GC timestamp after store recreation"),
        written
    );
}

/// 普通与受限统计查询共享投影、过滤与排序语义。
#[test]
fn normal_and_restricted_stats_queries_share_projection_filter_and_order() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table projection_a(a int)", Vec::new());
    testkit.MustExec("create table projection_b(a int)", Vec::new());
    testkit.MustExec("analyze table projection_a, projection_b", Vec::new());
    let first = domain
        .stats_table("test", "projection_a")
        .expect("projection_a metadata")
        .0
        .table_id;
    let second = domain
        .stats_table("test", "projection_b")
        .expect("projection_b metadata")
        .0
        .table_id;
    let sql = format!(
        "select modify_count, table_id from mysql.stats_meta where table_id >= {} order by table_id desc",
        first.min(second)
    );
    let expected = vec![
        vec!["0".to_owned(), first.max(second).to_string()],
        vec!["0".to_owned(), first.min(second).to_string()],
    ];

    assert_eq!(testkit.MustQuery(&sql, Vec::new()).Rows(), expected);
    assert_eq!(
        domain
            .restricted_stats_query(&sql, &[])
            .expect("restricted projected query"),
        expected
    );
}
