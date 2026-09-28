// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Ports of `pkg/statistics/asyncload/async_load_test.go`.
//
// Every Go test drives the queue indirectly: it opens a mock store/domain,
// runs real SQL (`CREATE TABLE`, `ANALYZE`, a predicate `SELECT`, then a
// `DROP`), waits for `AsyncLoadHistogramNeededItems` to be populated, calls
// `handle.LoadNeededHistograms(is)` and asserts the queue was cleaned up.
//
// Two adaptations to this workspace, neither of which drops Go logic:
//
// * `handle.LoadNeededHistograms(dom.InfoSchema())` is `Domain`-owned here,
//   so the tests call `domain.load_needed_histograms()`.
// * Go's `ANALYZE` executor refreshes the statistics cache from storage on
//   its way out, which returns every histogram in the all-evicted state and
//   is what makes the following predicate `SELECT` queue an async load. This
//   runtime leaves the freshly analyzed payload in the cache, so the tests
//   reproduce that refresh explicitly with `evict_cached_histograms`.
//
// `AsyncLoadHistogramNeededItems` is process-global while each test builds
// its own store (and therefore reuses table IDs), so the tests serialize on
// `QUEUE_GUARD` and start from an empty queue, mirroring Go's one-test-at-a-
// time package execution.
//
// 异步加载直方图测试（对应 Go `async_load_test.go`）。
//
// 通过真实 SQL（建表、ANALYZE、带谓词 SELECT、再 DROP）间接驱动
// `AsyncLoadHistogramNeededItems` 队列，再调用 `load_needed_histograms` 并断言队列清空。
// ANALYZE 后需显式 `evict_cached_histograms` 以复现 Go 侧“缓存仅留成员、需异步加载”的状态；
// 进程级队列用 `QUEUE_GUARD` 串行化，避免表 ID 复用互相干扰。

use astersql_domain::Domain;
use astersql_statistics_asyncload::AsyncLoadHistogramNeededItems;
use astersql_testkit::TestKit;
use astersql_testkit::db_driver::DbValue;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// 进程级互斥锁：串行化依赖全局异步加载队列的测试。
static QUEUE_GUARD: Mutex<()> = Mutex::new(());

/// 串行化测试并清空异步加载队列后返回守卫。
/// Serializes the tests and hands back an empty async-load queue.
fn acquire_queue() -> MutexGuard<'static, ()> {
    let guard = QUEUE_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for item in AsyncLoadHistogramNeededItems.AllItems() {
        AsyncLoadHistogramNeededItems.Delete(item.TableItemID);
    }
    guard
}

/// 在超时前轮询条件；对应 Go `require.Eventually`。
/// Go: `require.Eventually(t, condition, 5*time.Second, 2*time.Second, ...)`.
fn eventually(condition: impl Fn() -> bool, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if condition() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("{message}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 判断全局队列中是否仍有指定 table_id 的待加载项。
fn queue_holds_table(table_id: i64) -> bool {
    AsyncLoadHistogramNeededItems
        .AllItems()
        .iter()
        .any(|item| item.TableItemID.TableID == table_id)
}

/// 断言指定 table_id 已从异步加载队列中移除。
fn require_table_absent_from_queue(table_id: i64) {
    for item in AsyncLoadHistogramNeededItems.AllItems() {
        assert_ne!(
            table_id, item.TableItemID.TableID,
            "table {table_id} must be removed from the async load queue"
        );
    }
}

/// 复现 Go ANALYZE 出口处的统计缓存刷新：存储保留完整载荷，缓存仅留直方图成员，
/// 使后续谓词查询必须请求异步加载。
/// Reproduces the stats-cache refresh Go's `ANALYZE` executor performs: the
/// analyzed payload stays in storage while the cache keeps only histogram
/// membership, so the next predicate query has to request a load.
fn evict_cached_histograms(domain: &Domain) {
    domain
        .stats_handle()
        .lock()
        .expect("domain statistics handle")
        .clear();
    domain
        .update_stats()
        .expect("reload lite statistics after ANALYZE");
}

/// 五个 Go 对齐测试的公共前缀：建库表、插入、flush、ANALYZE，并返回 table_id。
/// Common prologue of all five Go tests, up to and including the `ANALYZE`.
fn setup(create_table: &str, analyze: &str) -> (Arc<Domain>, TestKit, i64) {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    // 关闭同步加载等待，迫使统计走异步队列路径。
    // Turn off the sync load.
    tk.MustExec("SET @@tidb_stats_load_sync_wait = 0", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("DROP TABLE IF EXISTS t1", Vec::new());
    tk.MustExec(create_table, Vec::new());
    tk.MustExec(
        "INSERT INTO t1 VALUES (1,3,0), (2,2,0), (3,2,0)",
        Vec::new(),
    );
    tk.MustExec("flush stats_delta *.*", Vec::new());
    domain.update_stats().expect("handle.Update");
    tk.MustExec(analyze, Vec::new());
    let table_id = domain
        .table_by_name("test", "t1")
        .expect("table t1 metadata")
        .ID;
    (domain, tk, table_id)
}

// go_test_load_column_statistics_after_table_drop 对应 Go 的
// TestLoadColumnStatisticsAfterTableDrop：列统计入队后表被删除，
// LoadNeededHistograms 必须成功返回并清空该表的待加载项。
#[test]
fn go_test_load_column_statistics_after_table_drop() {
    let _guard = acquire_queue();
    let (domain, mut tk, table_id) = setup(
        "CREATE TABLE t1 (a INT, b INT, c INT)",
        "ANALYZE TABLE t1 ALL COLUMNS",
    );
    evict_cached_histograms(&domain);
    // This will add the table to the AsyncLoadHistogramNeededItems.
    tk.MustExec("SELECT * FROM t1 WHERE b = 2", Vec::new());
    eventually(
        || queue_holds_table(table_id),
        &format!("table {table_id} should be in the items"),
    );

    // Drop the table.
    tk.MustExec("DROP TABLE t1", Vec::new());
    domain
        .load_needed_histograms()
        .expect("LoadNeededHistograms after DROP TABLE");

    require_table_absent_from_queue(table_id);
}

// go_test_load_statistics_after_column_drop 对应 Go 的
// TestLoadStatisticsAfterColumnDrop：谓词列被 DROP COLUMN 删除后，
// LoadNeededHistograms 依然成功并把该项目移出队列。
#[test]
fn go_test_load_statistics_after_column_drop() {
    let _guard = acquire_queue();
    let (domain, mut tk, table_id) = setup(
        "CREATE TABLE t1 (a INT, b INT, c INT)",
        "ANALYZE TABLE t1 ALL COLUMNS",
    );
    evict_cached_histograms(&domain);
    // This will add the table to the AsyncLoadHistogramNeededItems.
    tk.MustExec("SELECT * FROM t1 WHERE b = 2", Vec::new());
    eventually(
        || queue_holds_table(table_id),
        &format!("table {table_id} should be in the items"),
    );

    // Drop the column.
    tk.MustExec("ALTER TABLE t1 DROP COLUMN b", Vec::new());
    domain
        .load_needed_histograms()
        .expect("LoadNeededHistograms after DROP COLUMN");

    require_table_absent_from_queue(table_id);
}

// go_test_load_index_statistics_after_table_drop 对应 Go 的
// TestLoadIndexStatisticsAfterTableDrop：索引统计入队后整表被删除。
#[test]
fn go_test_load_index_statistics_after_table_drop() {
    let _guard = acquire_queue();
    let (domain, mut tk, table_id) = setup(
        "CREATE TABLE t1 (a INT, b INT, c INT, INDEX idx_b (b))",
        "ANALYZE TABLE t1 ALL COLUMNS",
    );
    evict_cached_histograms(&domain);
    // This will add the table to the AsyncLoadHistogramNeededItems.
    tk.MustExec("SELECT * FROM t1 WHERE b = 2", Vec::new());
    eventually(
        || queue_holds_table(table_id),
        &format!("table {table_id} should be in the items"),
    );

    // Drop the table.
    tk.MustExec("DROP TABLE t1", Vec::new());
    domain
        .load_needed_histograms()
        .expect("LoadNeededHistograms after DROP TABLE");

    require_table_absent_from_queue(table_id);
}

// go_test_load_statistics_after_index_drop 对应 Go 的
// TestLoadStatisticsAfterIndexDrop：索引统计入队后索引被删除。
#[test]
fn go_test_load_statistics_after_index_drop() {
    let _guard = acquire_queue();
    let (domain, mut tk, table_id) = setup(
        "CREATE TABLE t1 (a INT, b INT, c INT, INDEX idx_b (b))",
        "ANALYZE TABLE t1 ALL COLUMNS",
    );
    evict_cached_histograms(&domain);
    // This will add the table to the AsyncLoadHistogramNeededItems.
    tk.MustExec("SELECT * FROM t1 WHERE b = 2", Vec::new());
    eventually(
        || queue_holds_table(table_id),
        &format!("table {table_id} should be in the items"),
    );

    // Drop the index.
    tk.MustExec("ALTER TABLE t1 DROP INDEX idx_b", Vec::new());
    domain
        .load_needed_histograms()
        .expect("LoadNeededHistograms after DROP INDEX");

    require_table_absent_from_queue(table_id);
}

// go_test_load_corrupted_statistics 对应 Go 的 TestLoadCorruptedStatistics：
// mysql.stats_buckets 的边界被改写成非法字节后，LoadNeededHistograms 仍必须
// 成功返回并清空队列，而不是报错或 panic。
#[test]
fn go_test_load_corrupted_statistics() {
    let _guard = acquire_queue();
    // Only collect the histogram buckets.
    let (domain, mut tk, table_id) = setup(
        "CREATE TABLE t1 (a INT, b INT, c INT, INDEX idx_b (b))",
        "ANALYZE TABLE t1 ALL COLUMNS WITH 0 TOPN",
    );
    // Corrupt the statistics.
    tk.MustExec(
        "UPDATE mysql.stats_buckets SET upper_bound = 'who knows what it is' WHERE table_id = ?",
        vec![DbValue::I64(table_id)],
    );
    evict_cached_histograms(&domain);
    // This will add the table to the AsyncLoadHistogramNeededItems.
    tk.MustExec("SELECT * FROM t1 WHERE b = 2", Vec::new());
    eventually(
        || queue_holds_table(table_id),
        &format!("table {table_id} should be in the items"),
    );

    domain
        .load_needed_histograms()
        .expect("LoadNeededHistograms over corrupted buckets");

    require_table_absent_from_queue(table_id);
}
