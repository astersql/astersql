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

// 统计 GC 相关测试。
//
// 可运行部分覆盖 `batch_count` 与 `current_ts` 单调性；其余 Go 集成用例以
// 草稿字符串归档，描述 drop 索引/列/表后系统表清理语义。

const _GO_DRAFT_ARCHIVE: &str = r########################################"
// stats_meta、stats_histograms、stats_buckets、column_stats_usage 的清理验证；

// test_gc_stats 对应 Go 的 TestGCStats。
// 它依次 drop index、drop column、drop table，验证 GCStats 清理对应统计系统表。
#[test]
fn test_gc_stats() {
    let (store, dom) = testkit::create_mock_store_and_domain();
    let mut tk = testkit::new_test_kit(store);
    tk.must_exec("set @@tidb_analyze_version = 2");
    tk.must_exec("use test");
    tk.must_exec("create table t(a int, b int, index idx(a, b), index idx_a(a))");
    tk.must_exec("insert into t values (1,1),(2,2),(3,3)");
    tk.must_exec("analyze table t with 0 topn");

    tk.must_exec("alter table t drop index idx");
    tk.must_query("select count(*) from mysql.stats_histograms").check(rows("4"));
    tk.must_query("select count(*) from mysql.stats_buckets").check(rows("12"));
    let h = dom.stats_handle();
    let ddl_lease = duration::zero();
    assert!(h.gc_stats(dom.info_schema(), ddl_lease).is_ok());
    tk.must_query("select count(*) from mysql.stats_histograms").check(rows("3"));
    tk.must_query("select count(*) from mysql.stats_buckets").check(rows("9"));

    tk.must_exec("alter table t drop index idx_a");
    tk.must_exec("alter table t drop column a");
    assert!(h.gc_stats(dom.info_schema(), ddl_lease).is_ok());
    tk.must_query("select count(*) from mysql.stats_histograms").check(rows("1"));
    tk.must_query("select count(*) from mysql.stats_buckets").check(rows("3"));

    tk.must_exec("drop table t");
    assert!(h.gc_stats(dom.info_schema(), ddl_lease).is_ok());
    tk.must_query("select count(*) from mysql.stats_meta").check(rows("1"));
    tk.must_query("select count(*) from mysql.stats_histograms").check(rows("0"));
    tk.must_query("select count(*) from mysql.stats_buckets").check(rows("0"));
    assert!(h.gc_stats(dom.info_schema(), ddl_lease).is_ok());
    tk.must_query("select count(*) from mysql.stats_meta").check(rows("0"));
}

// test_gc_partition 对应 Go 的 TestGCPartition。
// 静态裁剪模式下验证分区表删除索引、删除列、删除表后的分区统计清理；FIXME 行为保持原注释。
#[test]
fn test_gc_partition() {
    let (store, dom) = testkit::create_mock_store_and_domain();
    let mut tk = testkit::new_test_kit(store);
    tk.must_exec("set @@tidb_analyze_version = 2");
    testkit::with_prune_mode(&mut tk, variable::Static, || {
        tk.must_exec("use test");
        tk.must_exec("create table t (a bigint(64), b bigint(64), index idx(a, b)) partition by range (a) (partition p0 values less than (3), partition p1 values less than (6))");
        tk.must_exec("insert into t values (1,2),(2,3),(3,4),(4,5),(5,6)");
        tk.must_exec("analyze table t with 0 topn");
        tk.must_query("select count(*) from mysql.stats_histograms").check(rows("6"));
        tk.must_query("select count(*) from mysql.stats_buckets").check(rows("15"));

        let h = dom.stats_handle();
        let ddl_lease = duration::zero();
        tk.must_exec("alter table t drop index idx");
        assert!(h.gc_stats(dom.info_schema(), ddl_lease).is_ok());
        tk.must_query("select count(*) from mysql.stats_histograms").check(rows("4"));
        tk.must_query("select count(*) from mysql.stats_buckets").check(rows("10"));

        tk.must_exec("alter table t drop column b");
        assert!(h.gc_stats(dom.info_schema(), ddl_lease).is_ok());
        tk.must_query("select count(*) from mysql.stats_histograms").check(rows("2"));
        tk.must_query("select count(*) from mysql.stats_buckets").check(rows("5"));

        tk.must_exec("drop table t");
        assert!(h.gc_stats(dom.info_schema(), ddl_lease).is_ok());
        tk.must_query("select count(*) from mysql.stats_meta").check(rows("3"));
        tk.must_query("select count(*) from mysql.stats_histograms").check(rows("0"));
        tk.must_query("select count(*) from mysql.stats_buckets").check(rows("0"));
        // FIXME(#68076): 剩下的是逻辑表 meta-only stats 行，常规 GC 版本窗口扫描不会在 drop 后再次访问它。
        assert!(h.gc_stats(dom.info_schema(), ddl_lease).is_ok());
        tk.must_query("select count(*) from mysql.stats_meta").check(rows("1"));
    });
}

// test_gc_column_stats_usage 对应 Go 的 TestGCColumnStatsUsage。
// 它验证 drop column 和 drop table 后，GCStats 会清理 column_stats_usage 中对应的列使用记录。
#[test]
fn test_gc_column_stats_usage() {
    let (store, dom) = testkit::create_mock_store_and_domain();
    let mut tk = testkit::new_test_kit(store);
    tk.must_exec("use test");
    tk.must_exec("create table t(a int, b int, c int)");
    tk.must_exec("insert into t values (1,1,1),(2,2,2),(3,3,3)");
    analyzehelper::trigger_predicate_columns_collection(&mut tk, "t", "a", "b", "c");
    tk.must_exec("analyze table t");
    tk.must_query("select count(*) from mysql.column_stats_usage").check(rows("3"));
    tk.must_exec("alter table t drop column a");
    tk.must_query("select count(*) from mysql.column_stats_usage").check(rows("3"));
    let h = dom.stats_handle();
    assert!(h.gc_stats(dom.info_schema(), duration::zero()).is_ok());
    tk.must_query("select count(*) from mysql.column_stats_usage").check(rows("2"));
    tk.must_exec("drop table t");
    tk.must_query("select count(*) from mysql.column_stats_usage").check(rows("2"));
    assert!(h.gc_stats(dom.info_schema(), duration::zero()).is_ok());
    tk.must_query("select count(*) from mysql.column_stats_usage").check(rows("0"));
}

// test_delete_analyze_jobs 对应 Go 的 TestDeleteAnalyzeJobs。
// analyze 后 show analyze status 有 1 行，DeleteAnalyzeJobs 使用未来时间删除后应为 0 行。
#[test]
fn test_delete_analyze_jobs() {
    let (store, dom) = testkit::create_mock_store_and_domain();
    let mut tk = testkit::new_test_kit(store);
    tk.must_exec("use test");
    tk.must_exec("create table t(a int, b int)");
    tk.must_exec("insert into t values (1,2),(3,4)");
    tk.must_exec("analyze table t");
    assert_eq!(1, tk.must_query("show analyze status").rows().len());
    assert!(dom.stats_handle().delete_analyze_jobs(time::now() + duration::seconds(1)).is_ok());
    assert_eq!(0, tk.must_query("show analyze status").rows().len());
}

// test_extrem_case_of_gc 对应 Go 的 TestExtremCaseOfGC，函数名沿用原拼写。
// 当 stats_histograms 没有记录但表仍存在时，不应删除 mysql.stats_meta 中的记录。
#[test]
fn test_extrem_case_of_gc() {
    let (store, dom) = testkit::create_mock_store_and_domain();
    let mut tk = testkit::new_test_kit(store);
    tk.must_exec("use test");
    tk.must_exec("create table t(a int, b int)");
    tk.must_exec("insert into t values (1,2),(3,4)");
    tk.must_exec("analyze table t");
    let tbl = dom.info_schema().table_by_name(context::todo(), ast::new_ci_str("test"), ast::new_ci_str("t")).unwrap();
    let tid = tbl.meta().id;
    assert_eq!(1, tk.must_query("select * from mysql.stats_meta where table_id = ?", tid).rows().len());
    assert_eq!(2, tk.must_query("select * from mysql.stats_histograms where table_id = ?", tid).rows().len());
    let h = dom.stats_handle();
    failpoint::enable("github.com/pingcap/tidb/pkg/statistics/handle/storage/injectGCStatsLastTSOffset", "return(0)").unwrap();
    // Go ignores GCStats return value.
    let _ = h.gc_stats(dom.info_schema(), duration::seconds(3));
    assert_eq!(1, tk.must_query("select * from mysql.stats_meta where table_id = ?", tid).rows().len());
    failpoint::disable("github.com/pingcap/tidb/pkg/statistics/handle/storage/injectGCStatsLastTSOffset").unwrap();
}
"########################################;

/// 空输入、整除与非整除批次边界。
#[test]
fn canonical_stats_gc_batch_count_covers_empty_exact_and_partial_batches() {
    assert_eq!(crate::batch_count(0, 100), 0);
    assert_eq!(crate::batch_count(200, 100), 2);
    assert_eq!(crate::batch_count(201, 100), 3);
}

/// 多线程并发调用 `current_ts` 必须严格递增、无重复。
#[test]
fn current_ts_is_strictly_monotonic_under_concurrency() {
    let timestamps = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let timestamps = std::sync::Arc::clone(&timestamps);
        workers.push(std::thread::spawn(move || {
            for _ in 0..256 {
                timestamps
                    .lock()
                    .expect("timestamp result lock poisoned")
                    .push(crate::current_ts());
            }
        }));
    }
    for worker in workers {
        worker.join().expect("timestamp worker");
    }
    let mut timestamps = timestamps
        .lock()
        .expect("timestamp result lock poisoned")
        .clone();
    timestamps.sort_unstable();
    assert_eq!(timestamps.len(), 8 * 256);
    assert!(
        timestamps.windows(2).all(|pair| pair[0] < pair[1]),
        "current_ts returned duplicate or decreasing values"
    );
}

#[derive(Default)]
struct StartTimestampStore {
    scan_upper_bound: std::sync::Mutex<Option<u64>>,
}

impl crate::SqlStore for StartTimestampStore {
    fn start_ts(&self) -> Result<u64, crate::Error> {
        Ok(u64::MAX - 1)
    }

    fn execute(&self, _sql: &str) -> Result<Vec<crate::Row>, crate::Error> {
        Ok(Vec::new())
    }

    fn gc_meta_ids(
        &self,
        _minimum_version: u64,
        maximum_version: u64,
    ) -> Result<Vec<i64>, crate::Error> {
        *self
            .scan_upper_bound
            .lock()
            .expect("GC scan bound lock poisoned") = Some(maximum_version);
        Ok(Vec::new())
    }

    fn gc_timestamp(&self, _variable_name: &str) -> Result<Option<String>, crate::Error> {
        Ok(None)
    }
}

struct EmptyCatalog;

impl crate::StatsCatalog for EmptyCatalog {
    fn lease(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }

    fn table_exists(&self, _physical_id: i64) -> bool {
        false
    }

    fn histogram_exists(&self, _physical_id: i64, _histogram_id: i64, _is_index: bool) -> bool {
        false
    }
}

#[test]
fn gc_uses_the_store_transaction_clock_for_its_scan_window() {
    let store = StartTimestampStore::default();
    crate::gc_stats(&store, &EmptyCatalog, std::time::Duration::ZERO)
        .expect("statistics GC succeeds");
    assert_eq!(
        *store
            .scan_upper_bound
            .lock()
            .expect("GC scan bound lock poisoned"),
        Some(u64::MAX)
    );
}
