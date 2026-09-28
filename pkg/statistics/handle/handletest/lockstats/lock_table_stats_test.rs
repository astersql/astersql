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

// 表级 LOCK/UNLOCK STATS 集成测试（对应 Go `lock_table_stats_test.go`）。
//
// 验证：锁定后 ANALYZE 跳过且统计不变；历史统计 meta 在锁定期不写入；
// 重复加锁/解锁告警；多表批量锁定；DROP/TRUNCATE 后锁信息 GC；
// 分区表整表锁定与解锁时全局 count 回写；锁定期 delta 可为负。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Barrier, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use astersql_domain::Domain;
use astersql_meta_model::TableInfo;
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{TestKit, result::Result};

use crate::main_test::setup_common_tests;

/// 临时开启统计缓存内存配额的配置守卫；Drop 时恢复原配置。
struct StatsCacheConfigGuard {
    /// 配置互斥锁，避免并行测例互相覆盖全局 config。
    _lock: MutexGuard<'static, ()>,
    /// 退出时调用的配置恢复闭包。
    restore: Option<Box<dyn FnOnce()>>,
}

impl StatsCacheConfigGuard {
    /// 启用 `enable_stats_cache_mem_quota`，并持有进程级配置锁。
    fn enable() -> Self {
        static CONFIG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = CONFIG_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("statistics cache config lock poisoned");
        let restore = astersql_config::restore_func();
        astersql_config::update_global(|config| {
            config.performance.enable_stats_cache_mem_quota = true;
        });
        Self {
            _lock: lock,
            restore: Some(Box::new(restore)),
        }
    }
}

impl Drop for StatsCacheConfigGuard {
    fn drop(&mut self) {
        if let Some(restore) = self.restore.take() {
            restore();
        }
    }
}

/// 查询 `mysql.stats_table_locked` 锁记录条数的 SQL。
pub(super) const SELECT_TABLE_LOCK_SQL: &str = "select count(*) from mysql.stats_table_locked";

/// 执行 SQL，失败则 panic（对齐 Go `MustExec`）。
pub(super) fn exec(testkit: &mut TestKit, sql: &str) {
    testkit.MustExec(sql, Vec::new());
}

/// 执行查询并返回结果集（对齐 Go `MustQuery`）。
pub(super) fn query(testkit: &TestKit, sql: &str) -> Result {
    testkit.MustQuery(sql, Vec::new())
}

/// 创建普通表 `t`，ANALYZE 一次，返回 store / Domain / TestKit / 表元数据。
pub(super) fn setup_table() -> (Arc<AnalyzeStatsStore>, Arc<Domain>, TestKit, TableInfo) {
    setup_common_tests();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store.clone());
    exec(&mut testkit, "set @@tidb_analyze_version = 2");
    exec(&mut testkit, "use test");
    exec(&mut testkit, "drop table if exists t");
    exec(
        &mut testkit,
        "create table t(a int, b varchar(10), index idx_b (b))",
    );
    exec(&mut testkit, "analyze table test.t");
    let table = domain
        .table_by_name("test", "t")
        .expect("typed InfoSchema test.t metadata")
        .as_ref()
        .clone();
    (store, domain, testkit, table)
}

/// 读取 `SHOW WARNINGS` 的全部行。
pub(super) fn warning_rows(testkit: &TestKit) -> Vec<Vec<String>> {
    query(testkit, "show warnings").Rows()
}

/// 断言最近一条警告为指定文案（级别 Warning，错误码 1105）。
pub(super) fn assert_warning(testkit: &TestKit, expected: &str) {
    assert_eq!(
        warning_rows(testkit),
        vec![vec![
            "Warning".to_owned(),
            "1105".to_owned(),
            expected.to_owned()
        ]]
    );
}

/// 断言 `SHOW WARNINGS` 中包含指定告警，允许 ANALYZE 额外产生 Note。
///
/// 对齐 Go `requireWarningContains`：该辅助断言用于 ANALYZE 的跳过告警，
/// 不能把同一语句产生的采样率 Note 误判为失败。
pub(super) fn assert_warning_contains(testkit: &TestKit, expected: &str) {
    assert!(
        warning_rows(testkit).iter().any(|row| {
            row.len() == 3 && row[0] == "Warning" && row[1] == "1105" && row[2] == expected
        }),
        "warning {expected:?} not found in {:?}",
        warning_rows(testkit)
    );
}

/// 断言指定物理 ID 的列统计均已初始化（`IsStatsInitialized`）。
pub(super) fn assert_columns_initialized(domain: &Domain, physical_id: i64) {
    let stats = domain
        .stats_context()
        .persisted_physical_stats(physical_id)
        .expect("physical statistics");
    assert!(
        !stats.columns.is_empty(),
        "table {physical_id} has no column statistics"
    );
    for (column_id, column) in &stats.columns {
        assert!(
            column.IsStatsInitialized(),
            "column {column_id} of table {physical_id} is not initialized"
        );
    }
}

/// 轮询等待列统计初始化完成（最多约 1 秒），用于异步落盘场景。
fn assert_columns_eventually_initialized(domain: &Domain, physical_id: i64) {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let initialized = domain
            .stats_context()
            .persisted_physical_stats(physical_id)
            .is_some_and(|stats| {
                !stats.columns.is_empty()
                    && stats
                        .columns
                        .values()
                        .all(|column| column.IsStatsInitialized())
            });
        if initialized {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "column statistics for table {physical_id} were not initialized"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

/// 表级锁定：ANALYZE 跳过、统计不变；锁定期不写历史 meta；解锁后可更新。
#[test]
fn test_lock_and_unlock_table_stats() {
    let (_store, domain, mut testkit, table) = setup_table();
    let original = domain
        .stats_context()
        .persisted_physical_stats(table.ID)
        .expect("initial table statistics");
    assert_columns_initialized(&domain, table.ID);

    exec(&mut testkit, "set @@tidb_enable_historical_stats = on");
    exec(&mut testkit, "lock stats t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["1"]]);

    exec(&mut testkit, "insert into t(a, b) values(1,'a')");
    exec(&mut testkit, "insert into t(a, b) values(2,'b')");
    exec(&mut testkit, "analyze table test.t");
    assert_warning_contains(&testkit, "skip analyze locked table: test.t");
    assert_eq!(
        domain
            .stats_context()
            .persisted_physical_stats(table.ID)
            .expect("locked table statistics"),
        original
    );
    let locked_tables = domain
        .stats_lock()
        .GetTableLockedAndClearForTest()
        .expect("query locked tables");
    assert_eq!(locked_tables.len(), 1);

    // 锁定期 flush 不应新增历史统计版本；用 failpoint 守卫确保路径可观测。
    exec(&mut testkit, "insert into t(a, b) values(3,'c')");
    let history_before_flush = domain.stats_context().history(table.ID).len();
    let historical_meta_guard = domain.EnablePanicWhenRecordingHistoricalStatsMetaForTest();
    exec(&mut testkit, "flush stats_delta *.*");
    assert_eq!(
        domain.stats_context().history(table.ID).len(),
        history_before_flush
    );
    drop(historical_meta_guard);
    exec(&mut testkit, "unlock stats t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["0"]]);

    exec(&mut testkit, "analyze table test.t");
    assert_eq!(
        domain
            .stats_context()
            .persisted_physical_stats(table.ID)
            .expect("unlocked table statistics")
            .realtime_count,
        3
    );
}

/// 历史 meta failpoint 必须在 TestKit worker 线程可见（跨线程传播）。
#[test]
fn historical_meta_failpoint_crosses_testkit_worker_thread() {
    let (_store, domain, mut testkit, _table) = setup_table();
    exec(&mut testkit, "set @@tidb_enable_historical_stats = on");
    exec(&mut testkit, "insert into t(a, b) values(1,'a')");
    let historical_meta_guard = domain.EnablePanicWhenRecordingHistoricalStatsMetaForTest();

    let flush = catch_unwind(AssertUnwindSafe(|| {
        exec(&mut testkit, "flush stats_delta *.*");
    }));

    assert!(
        flush.is_err(),
        "historical-meta failpoint must be visible in the TestKit worker thread"
    );
    drop(historical_meta_guard);
}

/// 多个 Domain 各自启用历史 meta failpoint，互不串扰（按 Domain 作用域）。
#[test]
fn concurrent_historical_meta_failpoint_guards_are_domain_scoped() {
    let (_first_store, first_domain, mut first, _) = setup_table();
    let (_second_store, second_domain, mut second, _) = setup_table();
    exec(&mut first, "set @@tidb_enable_historical_stats = on");
    exec(&mut second, "set @@tidb_enable_historical_stats = on");
    exec(&mut first, "insert into t(a, b) values(1,'first')");
    exec(&mut second, "insert into t(a, b) values(1,'second')");
    let first_guard = first_domain.EnablePanicWhenRecordingHistoricalStatsMetaForTest();
    let second_guard = second_domain.EnablePanicWhenRecordingHistoricalStatsMetaForTest();
    let barrier = Arc::new(Barrier::new(2));

    let first_barrier = Arc::clone(&barrier);
    let first_flush = thread::spawn(move || {
        first_barrier.wait();
        catch_unwind(AssertUnwindSafe(|| {
            exec(&mut first, "flush stats_delta *.*");
        }))
        .is_err()
    });
    let second_barrier = Arc::clone(&barrier);
    let second_flush = thread::spawn(move || {
        second_barrier.wait();
        catch_unwind(AssertUnwindSafe(|| {
            exec(&mut second, "flush stats_delta *.*");
        }))
        .is_err()
    });

    assert!(first_flush.join().expect("first failpoint worker"));
    assert!(second_flush.join().expect("second failpoint worker"));
    drop(first_guard);
    drop(second_guard);
}

/// 分区表整表锁定：全局+各分区共 3 条锁；ANALYZE 跳过；解锁后清空。
#[test]
fn test_lock_and_unlock_partitioned_table_stats() {
    let (_store, domain, mut testkit, table) =
        crate::lock_partition_stats_test::setup_partitioned_table();
    assert_columns_initialized(&domain, table.ID);
    exec(&mut testkit, "lock stats t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["3"]]);
    assert_eq!(query(&testkit, "show stats_locked").Rows().len(), 3);

    exec(&mut testkit, "analyze table test.t");
    assert_warning_contains(
        &testkit,
        "skip analyze locked tables: test.t partition (p0), test.t partition (p1)",
    );

    exec(&mut testkit, "unlock stats t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["0"]]);
    assert!(query(&testkit, "show stats_locked").Rows().is_empty());
    assert!(domain.stats_context().locked_table_ids().is_empty());
}

/// 重复锁定已锁表 / 解锁未锁表产生 skip 警告；中间解锁后 ANALYZE 可更新行数。
#[test]
fn test_lock_table_and_unlock_table_stats_repeatedly() {
    let (_store, domain, mut testkit, table) = setup_table();
    let original = domain
        .stats_context()
        .persisted_physical_stats(table.ID)
        .expect("initial table statistics");
    assert_columns_initialized(&domain, table.ID);
    exec(&mut testkit, "lock stats t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["1"]]);
    exec(&mut testkit, "insert into t(a, b) values(1,'a')");
    exec(&mut testkit, "insert into t(a, b) values(2,'b')");
    exec(&mut testkit, "analyze table test.t");
    assert_eq!(
        domain
            .stats_context()
            .persisted_physical_stats(table.ID)
            .expect("locked table statistics"),
        original
    );

    let locked = domain
        .stats_lock()
        .GetTableLockedAndClearForTest()
        .expect("query locked tables before repeated lock");
    exec(&mut testkit, "lock stats t");
    assert_warning(&testkit, "skip locking locked table: test.t");
    assert_eq!(
        domain
            .stats_lock()
            .GetTableLockedAndClearForTest()
            .expect("query locked tables after repeated lock"),
        locked
    );

    exec(&mut testkit, "unlock stats t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["0"]]);
    exec(&mut testkit, "analyze table test.t");
    assert_eq!(
        domain
            .stats_context()
            .persisted_physical_stats(table.ID)
            .expect("unlocked table statistics")
            .realtime_count,
        2
    );

    exec(&mut testkit, "unlock stats t");
    assert_warning(&testkit, "skip unlocking unlocked table: test.t");
}

/// 多表批量 LOCK/UNLOCK：两表统计均冻结，解锁后各自 realtime_count 更新。
#[test]
fn test_lock_and_unlock_tables_stats() {
    let _config = StatsCacheConfigGuard::enable();
    setup_common_tests();
    assert!(
        astersql_config::get_global_config()
            .performance
            .enable_stats_cache_mem_quota
    );
    let (_store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(_store.clone());
    exec(&mut testkit, "set @@tidb_analyze_version = 2");
    exec(&mut testkit, "use test");
    exec(&mut testkit, "drop table if exists t1");
    exec(&mut testkit, "drop table if exists t2");
    exec(
        &mut testkit,
        "create table t1(a int, b varchar(10), index idx_b (b))",
    );
    exec(
        &mut testkit,
        "create table t2(a int, b varchar(10), index idx_b (b))",
    );
    exec(&mut testkit, "analyze table test.t1, test.t2");
    let table1 = domain
        .table_by_name("test", "t1")
        .expect("typed InfoSchema test.t1")
        .as_ref()
        .clone();
    let table2 = domain
        .table_by_name("test", "t2")
        .expect("typed InfoSchema test.t2")
        .as_ref()
        .clone();
    let original1 = domain
        .stats_context()
        .persisted_physical_stats(table1.ID)
        .expect("t1 statistics");
    let original2 = domain
        .stats_context()
        .persisted_physical_stats(table2.ID)
        .expect("t2 statistics");
    assert_columns_eventually_initialized(&domain, table1.ID);
    assert_columns_eventually_initialized(&domain, table2.ID);

    exec(&mut testkit, "lock stats t1, t2");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["2"]]);
    exec(&mut testkit, "insert into t1(a, b) values(1,'a')");
    exec(&mut testkit, "insert into t1(a, b) values(2,'b')");
    exec(&mut testkit, "insert into t2(a, b) values(1,'a')");
    exec(&mut testkit, "insert into t2(a, b) values(2,'b')");
    exec(&mut testkit, "analyze table test.t1, test.t2");
    assert_warning_contains(&testkit, "skip analyze locked tables: test.t1, test.t2");
    assert_eq!(
        domain
            .stats_context()
            .persisted_physical_stats(table1.ID)
            .expect("locked t1"),
        original1
    );
    assert_eq!(
        domain
            .stats_context()
            .persisted_physical_stats(table2.ID)
            .expect("locked t2"),
        original2
    );
    let locked_tables = domain
        .stats_lock()
        .GetTableLockedAndClearForTest()
        .expect("query multi-table locks");
    assert_eq!(locked_tables.len(), 2);

    exec(&mut testkit, "unlock stats test.t1, test.t2");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["0"]]);
    exec(&mut testkit, "analyze table test.t1, test.t2");
    assert_eq!(
        domain
            .stats_context()
            .persisted_physical_stats(table1.ID)
            .expect("unlocked t1")
            .realtime_count,
        2
    );
    assert_eq!(
        domain
            .stats_context()
            .persisted_physical_stats(table2.ID)
            .expect("unlocked t2")
            .realtime_count,
        2
    );
}

/// DROP TABLE 后 GC 应清除该表的锁记录。
#[test]
fn test_drop_table_should_clean_up_lock_info() {
    let (_store, domain, mut testkit, table) = setup_table();
    assert_columns_initialized(&domain, table.ID);
    exec(&mut testkit, "lock stats t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["1"]]);
    exec(&mut testkit, "drop table t");
    domain
        .gc_stats(Duration::ZERO)
        .expect("GC dropped table statistics");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["0"]]);
}

/// TRUNCATE TABLE 后 GC 应清除该表的锁记录。
#[test]
fn test_truncate_table_should_clean_up_lock_info() {
    let (_store, domain, mut testkit, table) = setup_table();
    assert_columns_initialized(&domain, table.ID);
    exec(&mut testkit, "lock stats t");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["1"]]);
    exec(&mut testkit, "truncate table t");
    domain
        .gc_stats(Duration::ZERO)
        .expect("GC truncated table statistics");
    query(&testkit, SELECT_TABLE_LOCK_SQL).Check(vec![vec!["0"]]);
}

/// 分区表整表锁定：不可用分区级 UNLOCK；整表解锁后全局 count/modify_count 回写。
#[test]
fn test_unlock_partitioned_table_updates_global_count_correctly() {
    let (_store, domain, mut testkit, table) =
        crate::lock_partition_stats_test::setup_partitioned_table();
    exec(&mut testkit, "lock stats t");
    exec(&mut testkit, "insert into t(a, b) values(1,'a')");
    exec(&mut testkit, "insert into t(a, b) values(2,'b')");
    exec(&mut testkit, "analyze table test.t");
    assert_eq!(
        domain
            .stats_context()
            .persisted_physical_stats(table.ID)
            .expect("locked global statistics")
            .realtime_count,
        0
    );
    exec(&mut testkit, "flush stats_delta *.*");
    query(
        &testkit,
        "select count, modify_count, table_id from mysql.stats_table_locked order by table_id",
    )
    .Check(vec![
        vec!["0", "0", &table.ID.to_string()],
        vec![
            "2",
            "2",
            &table.Partition.as_ref().unwrap().Definitions[0]
                .ID
                .to_string(),
        ],
        vec![
            "0",
            "0",
            &table.Partition.as_ref().unwrap().Definitions[1]
                .ID
                .to_string(),
        ],
    ]);

    // 整表锁定时分区级 unlock 应被跳过。
    exec(&mut testkit, "unlock stats t partition p0, p1");
    assert_warning(
        &testkit,
        "skip unlocking partitions of locked table: test.t",
    );
    exec(&mut testkit, "unlock stats t");
    query(
        &testkit,
        &format!(
            "select count, modify_count from mysql.stats_meta where table_id = {}",
            table.ID
        ),
    )
    .Check(vec![vec!["2", "2"]]);
}

/// 锁定期内删除行可使 locked 表上的 count 为负；解锁后合并进 stats_meta。
#[test]
fn test_delta_in_lock_info_can_be_negative() {
    let (_store, _domain, mut testkit, table) =
        crate::lock_partition_stats_test::setup_partitioned_table();
    exec(&mut testkit, "insert into t(a, b) values(1,'a')");
    exec(&mut testkit, "insert into t(a, b) values(2,'b')");
    exec(&mut testkit, "flush stats_delta *.*");
    query(
        &testkit,
        &format!(
            "select count, modify_count from mysql.stats_meta where table_id = {}",
            table.ID
        ),
    )
    .Check(vec![vec!["2", "2"]]);

    exec(&mut testkit, "lock stats t");
    exec(&mut testkit, "delete from t where a = 1");
    exec(&mut testkit, "delete from t where a = 2");
    exec(&mut testkit, "flush stats_delta *.*");
    // 分区 p0 上 count 为 -2，modify_count 为 2。
    query(
        &testkit,
        "select count, modify_count, table_id from mysql.stats_table_locked order by table_id",
    )
    .Check(vec![
        vec!["0", "0", &table.ID.to_string()],
        vec![
            "-2",
            "2",
            &table.Partition.as_ref().unwrap().Definitions[0]
                .ID
                .to_string(),
        ],
        vec![
            "0",
            "0",
            &table.Partition.as_ref().unwrap().Definitions[1]
                .ID
                .to_string(),
        ],
    ]);
    exec(&mut testkit, "unlock stats t");
    query(
        &testkit,
        &format!(
            "select count, modify_count from mysql.stats_meta where table_id = {}",
            table.ID
        ),
    )
    .Check(vec![vec!["0", "4"]]);
}
