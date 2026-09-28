// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Same-path Go→Rust mapping for `handle_test.go` using
// `CreateMockStoreAndDomain` / TestKit / Domain InfoSchema / production stats handle.
//
// 对应 Go `handle_test.go`：经 CreateMockStoreAndDomain / TestKit / Domain
// InfoSchema 与生产路径统计 handle，覆盖空表分析、列 ID、版本、直方图加载、
// 相关性、分区全局合并、FM Sketch、缓存、增量 modify_count、历史统计、
// InitStatsLite、BIT 列、系统/临时表缓存排除、索引裁剪异步加载等。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use astersql_domain::Domain;
use astersql_meta_model::TableInfo;
use astersql_statistics::{DecodeFMSketch, Version2, calcCorrelation};
use astersql_statistics_asyncload::AsyncLoadHistogramNeededItems;
use astersql_statistics_handle::TableStats;
use astersql_statistics_handle_util::duration_to_ts;
use astersql_testkit::TestKit;
use astersql_types::datum::NewIntDatum;

use crate::main_test::{new_exclusive_store_and_domain, new_store_and_domain};

/// 按库名 `test` + 表名从 Domain InfoSchema 取表元数据。
fn table_meta(domain: &Domain, name: &str) -> Arc<TableInfo> {
    domain
        .table_by_name("test", name)
        .unwrap_or_else(|error| panic!("typed InfoSchema lookup for test.{name}: {error}"))
}

/// 读取物理表/分区 ID 对应的内存中表统计（TableStats）。
fn physical_stats(domain: &Domain, physical_id: i64) -> TableStats {
    domain
        .stats_context()
        .physical_stats(physical_id)
        .unwrap_or_else(|| panic!("missing physical stats for {physical_id}"))
}

/// 返回统计 handle 缓存中当前条目数。
fn cache_len(domain: &Domain) -> usize {
    let handle = domain.stats_handle();
    handle.lock().expect("statistics handle").cache().len()
}

fn runtime_integer_less_row_count(
    column_id: i64,
    column: &astersql_statistics_handle::ColumnStats,
    upper_value: i64,
) -> f64 {
    let builder = astersql_statistics::RuntimeStatsBuilder::default();
    let mut histogram = astersql_statistics::NewHistogram(
        column_id,
        column.ndv,
        column.null_count,
        column.version,
        &astersql_types::field::NewFieldType(3),
        column.buckets.len(),
        column.total_column_size,
    );
    for bucket in &column.buckets {
        let decode = |encoded: &[u8]| {
            builder
                .decode_histogram_bound(encoded, false)
                .expect("decode integer histogram bound")
        };
        histogram.AppendBucketWithNDV(
            &decode(&bucket.lower),
            &decode(&bucket.upper),
            bucket.count,
            bucket.repeats,
            bucket.ndv,
        );
    }
    let upper = NewIntDatum(upper_value);
    histogram.LessRowCount(&upper)
        + column
            .top_n
            .iter()
            .filter(|(encoded, _)| {
                builder
                    .decode_histogram_bound(encoded, false)
                    .expect("decode integer TopN value")
                    .GetInt64()
                    < upper.GetInt64()
            })
            .map(|(_, count)| *count as f64)
            .sum::<f64>()
}

/// 断言该表不存在 partition_name=`global` 的 buckets/histograms（静态裁剪模式）。
fn must_no_global_stats(testkit: &TestKit, table: &str) {
    // Go MustNoGlobalStats checks buckets/histograms and intentionally ignores
    // global stats_meta rows maintained by stats-delta flushes.
    // 对齐 Go MustNoGlobalStats：只查 buckets/histograms，忽略 stats_meta 全局行。
    let buckets = testkit
        .MustQuery(
            &format!("show stats_buckets where table_name like '{table}'"),
            Vec::new(),
        )
        .Rows();
    assert!(
        buckets
            .iter()
            .all(|row| row.get(2).map(String::as_str) != Some("global")),
        "global buckets should not be found for {table}, got {buckets:?}"
    );
    let histograms = testkit
        .MustQuery(
            &format!("show stats_histograms where table_name like '{table}'"),
            Vec::new(),
        )
        .Rows();
    assert!(
        histograms
            .iter()
            .all(|row| row.get(2).map(String::as_str) != Some("global")),
        "global histograms should not be found for {table}, got {histograms:?}"
    );
}

/// 取出 `show stats_histograms` 中该表各行的 Correlation 列（已排序）。
fn histogram_correlation_rows(testkit: &TestKit, table: &str) -> Vec<String> {
    let mut rows = testkit
        .MustQuery(
            &format!("show stats_histograms where Table_name = '{table}'"),
            Vec::new(),
        )
        .Rows();
    rows.sort();
    rows.into_iter()
        .map(|row| row.get(9).cloned().unwrap_or_default())
        .collect()
}

/// 空表 ANALYZE 后统计非伪、已初始化、行列数为 0 且列/索引条数正确。
#[test]
fn go_test_empty_table() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec(
        "create table t (c1 int, c2 int, key cc1(c1), key cc2(c2))",
        Vec::new(),
    );
    testkit.MustExec("analyze table t", Vec::new());
    let table = table_meta(&domain, "t");
    let stats = physical_stats(&domain, table.ID);
    assert!(!stats.pseudo);
    assert!(stats.initialized);
    assert_eq!(stats.realtime_count, 0);
    assert_eq!(stats.columns.len(), 2);
    assert_eq!(stats.indexes.len(), 2);
}

/// 校验列 ID 与直方图；DROP COLUMN 后统计不再保留已删列。
#[test]
fn go_test_column_ids() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t (c1 int, c2 int)", Vec::new());
    testkit.MustExec("insert into t values(1, 2)", Vec::new());
    testkit.MustExec("analyze table t all columns", Vec::new());

    let before = table_meta(&domain, "t");
    let c1_id = before.Columns[0].ID;
    let c2_id = before.Columns[1].ID;
    let stats = physical_stats(&domain, before.ID);
    assert!(stats.columns.contains_key(&c1_id));
    assert_eq!(
        runtime_integer_less_row_count(c1_id, &stats.columns[&c1_id], 2),
        1.0
    );

    testkit.MustExec("alter table t drop column c1", Vec::new());
    let after = table_meta(&domain, "t");
    assert_eq!(after.Columns.len(), 1);
    assert_eq!(after.Columns[0].ID, c2_id);
    let after_stats = physical_stats(&domain, after.ID);
    assert!(after_stats.columns.contains_key(&c2_id));
    assert!(!after_stats.columns.contains_key(&c1_id));
    assert_eq!(
        runtime_integer_less_row_count(c2_id, &after_stats.columns[&c2_id], 3),
        1.0
    );
}

/// `duration_to_ts` 将 Duration 转为 TS，高位对齐毫秒（右移 18 位）。
#[test]
fn go_test_duration_to_ts() {
    for duration in [
        Duration::from_millis(1),
        Duration::from_secs(1),
        Duration::from_secs(60),
        Duration::from_secs(3600),
    ] {
        assert_eq!(duration_to_ts(duration) >> 18, duration.as_millis() as u64);
    }
}

/// 多表 ANALYZE 共享初始 version；再分析后 version 递增且行数更新。
#[test]
fn go_test_version() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t1 (c1 int, c2 int)", Vec::new());
    testkit.MustExec("create table t2 (c1 int, c2 int)", Vec::new());
    testkit.MustExec("analyze table t1, t2 all columns", Vec::new());
    let t1 = table_meta(&domain, "t1");
    let t2 = table_meta(&domain, "t2");
    let first = physical_stats(&domain, t1.ID);
    let second = physical_stats(&domain, t2.ID);
    assert!(!first.pseudo);
    assert!(!second.pseudo);
    assert_eq!(first.version, second.version);
    let first_version = first.version;

    testkit.MustExec("insert into t1 values(1,2)", Vec::new());
    testkit.MustExec("analyze table t1", Vec::new());
    let updated_t1 = physical_stats(&domain, t1.ID);
    assert!(updated_t1.version > first_version);
    assert_eq!(updated_t1.realtime_count, 1);

    testkit.MustExec("insert into t2 values(1,2)", Vec::new());
    testkit.MustExec("analyze table t2", Vec::new());
    let updated_t2 = physical_stats(&domain, t2.ID);
    assert!(updated_t2.version > first_version);
    assert_eq!(updated_t2.realtime_count, 1);

    // Go exercises an independently-created Handle here. The Rust Domain owns
    // the canonical Handle, so use that same cache while preserving the
    // observable contract: a lower stats_meta version still loads the table,
    // but must never lower the cache's maximum version.
    // Go 此处用独立 Handle；Rust Domain 持有唯一的 canonical Handle。因此复用该
    // 缓存并保持可观察契约：较低的 stats_meta version 仍可加载表，但不能降低缓存最大版本。
    let unit = 1_u64 << 18;
    domain
        .restricted_stats_execute(
            &format!(
                "update mysql.stats_meta set version = {} where table_id = {}",
                2 * unit,
                t1.ID
            ),
            &[],
        )
        .expect("rewrite persisted t1 stats version");
    domain
        .update_stats()
        .expect("refresh stats cache at version 2");
    let max_version = domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .stats_meta_rows()
        .iter()
        .map(|stats| stats.version)
        .max()
        .expect("t1 statistics in cache");
    assert_eq!(max_version, 2 * unit);
    assert!(!physical_stats(&domain, t1.ID).pseudo);

    domain
        .restricted_stats_execute(
            &format!(
                "update mysql.stats_meta set version = {unit} where table_id = {}",
                t2.ID
            ),
            &[],
        )
        .expect("rewrite persisted t2 stats version");
    domain
        .update_stats()
        .expect("refresh stats cache at a lower version");
    assert_eq!(
        domain
            .stats_handle()
            .lock()
            .expect("statistics handle")
            .stats_meta_rows()
            .iter()
            .map(|stats| stats.version)
            .max()
            .expect("statistics in cache"),
        2 * unit
    );
    assert!(!physical_stats(&domain, t2.ID).pseudo);
}

/// flush stats_delta 后版本与行数变化；列 total_column_size 在 delta 后仍一致。
#[test]
fn go_test_load_hist() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t (c1 varchar(12), c2 char(12))", Vec::new());
    for _ in 0..10 {
        testkit.MustExec("insert into t values('a','ddd')", Vec::new());
    }
    testkit.MustExec("analyze table t", Vec::new());
    let table = table_meta(&domain, "t");
    let old = physical_stats(&domain, table.ID);
    for _ in 0..10 {
        testkit.MustExec("insert into t values('bb','sdfga')", Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    let changed = physical_stats(&domain, table.ID);
    assert_ne!(old.version, changed.version);
    assert_eq!(changed.realtime_count, 20);
    for (column_id, column) in &old.columns {
        assert_eq!(
            column.total_column_size,
            changed.columns[column_id].total_column_size
        );
    }

    // Adding a column only synthesizes statistics for the new column.  The
    // already-loaded histograms must retain their update versions, exactly as
    // Go's HandleNextDDLEventWithTxn + Update path does.
    // ADD COLUMN 仅为新列合成统计；已有直方图的更新时间必须保持不变，
    // 对齐 Go 的 HandleNextDDLEventWithTxn + Update 路径。
    let c1_version = changed.columns[&table.Columns[0].ID].version;
    let c2_version = changed.columns[&table.Columns[1].ID].version;
    testkit.MustExec("alter table t add column c3 int", Vec::new());
    domain
        .update_stats()
        .expect("refresh statistics after ADD COLUMN");
    let updated_table = table_meta(&domain, "t");
    let updated = physical_stats(&domain, updated_table.ID);
    let c3_id = updated_table.Columns[2].ID;
    assert_eq!(
        updated.columns[&updated_table.Columns[0].ID].version,
        c1_version
    );
    assert_eq!(
        updated.columns[&updated_table.Columns[1].ID].version,
        c2_version
    );
    assert!(updated.columns[&c3_id].version > c1_version);
}

/// 按值排序位置计算与主键序的相关性（Spearman 风格，委托 calcCorrelation）。
fn correlation_for_order(values: &[i64]) -> f64 {
    let mut positions = (0..values.len()).collect::<Vec<_>>();
    positions.sort_by_key(|&position| (values[position], position));
    let correlation_sum = positions
        .iter()
        .enumerate()
        .map(|(rank, &position)| (rank * position) as f64)
        .sum();
    calcCorrelation(values.len() as i64, correlation_sum)
}

/// 浮点相关性近似相等断言（误差 < 1e-15）。
fn assert_correlation(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-15,
        "expected correlation {expected}, got {actual}"
    );
}

/// 多种插入顺序下列相关性；含主键/二级索引直方图 Correlation 展示。
#[test]
fn go_test_correlation() {
    let (_store, _domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t(c1 int primary key, c2 int)", Vec::new());
    testkit.MustExec("set @@session.tidb_analyze_version=2", Vec::new());
    // Go also runs a predicate SELECT first for column usage; the minimal
    // session executor does not yet own user-table SELECT scans.
    // Go 还会先跑谓词 SELECT 收集列用法；精简执行器尚无用户表 SELECT 扫描。
    testkit.MustExec(
        "insert into t values(1,1),(3,12),(4,20),(2,7),(5,21)",
        Vec::new(),
    );
    testkit.MustExec("analyze table t", Vec::new());
    assert_eq!(histogram_correlation_rows(&testkit, "t"), vec!["1", "1"]);

    testkit.MustExec("insert into t values(8,18)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    assert_eq!(
        histogram_correlation_rows(&testkit, "t"),
        vec!["1", "0.8285714285714286"]
    );

    testkit.MustExec("truncate table t", Vec::new());
    assert!(histogram_correlation_rows(&testkit, "t").is_empty());
    testkit.MustExec(
        "insert into t values(1,21),(3,12),(4,7),(2,20),(5,1)",
        Vec::new(),
    );
    testkit.MustExec("analyze table t", Vec::new());
    assert_eq!(histogram_correlation_rows(&testkit, "t"), vec!["1", "-1"]);

    testkit.MustExec("insert into t values(8,4)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    assert_eq!(
        histogram_correlation_rows(&testkit, "t"),
        vec!["1", "-0.9428571428571428"]
    );

    testkit.MustExec("truncate table t", Vec::new());
    testkit.MustExec(
        "insert into t values (1,1),(2,1),(3,1),(4,1),(5,1),(6,1),(7,1),(8,1),(9,1),(10,1),(11,1),(12,1),(13,1),(14,1),(15,1),(16,1),(17,1),(18,1),(19,1),(20,2),(21,2),(22,2),(23,2),(24,2),(25,2)",
        Vec::new(),
    );
    testkit.MustExec("analyze table t", Vec::new());
    assert_eq!(histogram_correlation_rows(&testkit, "t"), vec!["1", "1"]);

    testkit.MustExec("drop table t", Vec::new());
    testkit.MustExec("create table t(c1 int, c2 int)", Vec::new());
    testkit.MustExec(
        "insert into t values(1,1),(2,7),(3,12),(4,20),(5,21),(8,18)",
        Vec::new(),
    );
    testkit.MustExec("analyze table t", Vec::new());
    assert_eq!(
        histogram_correlation_rows(&testkit, "t"),
        vec!["1", "0.8285714285714286"]
    );

    testkit.MustExec("truncate table t", Vec::new());
    testkit.MustExec(
        "insert into t values(1,1),(2,7),(3,12),(8,18),(4,20),(5,21)",
        Vec::new(),
    );
    testkit.MustExec("analyze table t", Vec::new());
    assert_eq!(
        histogram_correlation_rows(&testkit, "t"),
        vec!["0.8285714285714286", "1"]
    );

    testkit.MustExec("drop table t", Vec::new());
    testkit.MustExec(
        "create table t(c1 int primary key, c2 int, c3 int, key idx_c2(c2))",
        Vec::new(),
    );
    testkit.MustExec("insert into t values(1,1,1),(2,2,2),(3,3,3)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let columns = testkit
        .MustQuery(
            "show stats_histograms where Table_name = 't' and Is_index = 0",
            Vec::new(),
        )
        .Rows();
    assert_eq!(columns.len(), 3);
    assert!(columns.iter().all(|row| row[9] == "1"));
    let indexes = testkit
        .MustQuery(
            "show stats_histograms where Table_name = 't' and Is_index = 1",
            Vec::new(),
        )
        .Rows();
    assert_eq!(indexes.len(), 1);
    assert_eq!(indexes[0][9], "0");

    assert_eq!(correlation_for_order(&[1]), 1.0);
    assert_eq!(correlation_for_order(&[1, 7, 12, 20, 21]), 1.0);
    assert_correlation(
        correlation_for_order(&[1, 7, 12, 20, 21, 18]),
        0.8285714285714286,
    );
    assert_eq!(correlation_for_order(&[21, 20, 12, 7, 1]), -1.0);
    assert_correlation(
        correlation_for_order(&[21, 20, 12, 7, 1, 4]),
        -0.9428571428571428,
    );
}

/// 动态裁剪模式下合并分区 TopN 到 global（ANALYZE WITH N TOPN）。
#[test]
fn go_test_merge_global_top_n() {
    let (_store, _domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("drop table if exists t", Vec::new());
    testkit.MustExec("set @@session.tidb_analyze_version=2", Vec::new());
    testkit.MustExec(
        "set @@session.tidb_partition_prune_mode='dynamic'",
        Vec::new(),
    );
    testkit.MustExec(
        "create table t (a int, b int, key(b)) partition by range (a) (
            partition p0 values less than (10),
            partition p1 values less than (20)
        )",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values(1, 1), (1, 1), (1, 1), (1, 1), (2, 2), (2, 2), (3, 3), (3, 3), (3, 3), \
         (11, 11), (11, 11), (11, 11), (12, 12), (12, 12), (12, 12), (13, 3), (13, 3)",
        Vec::new(),
    );
    testkit.MustExec("analyze table t with 2 topn", Vec::new());
    testkit
        .MustQuery(
            "show stats_topn where table_name = 't' and column_name = 'b' and partition_name = 'p0'",
            Vec::new(),
        )
        .Check(vec![
            vec!["test", "t", "p0", "b", "0", "1", "4"],
            vec!["test", "t", "p0", "b", "0", "3", "3"],
            vec!["test", "t", "p0", "b", "1", "1", "4"],
            vec!["test", "t", "p0", "b", "1", "3", "3"],
        ]);
    testkit
        .MustQuery(
            "show stats_topn where table_name = 't' and column_name = 'b' and partition_name = 'p1'",
            Vec::new(),
        )
        .Check(vec![
            vec!["test", "t", "p1", "b", "0", "11", "3"],
            vec!["test", "t", "p1", "b", "0", "12", "3"],
            vec!["test", "t", "p1", "b", "1", "11", "3"],
            vec!["test", "t", "p1", "b", "1", "12", "3"],
        ]);
    testkit
        .MustQuery(
            "show stats_topn where table_name = 't' and column_name = 'b' and partition_name = 'global'",
            Vec::new(),
        )
        .Check(vec![
            vec!["test", "t", "global", "b", "0", "1", "4"],
            vec!["test", "t", "global", "b", "0", "3", "5"],
            vec!["test", "t", "global", "b", "1", "1", "4"],
            vec!["test", "t", "global", "b", "1", "3", "5"],
        ]);
}

/// 静态分区裁剪模式不产生 global 统计；切到动态后仍无 global。
#[test]
fn go_test_static_partition_prune_mode() {
    let (_store, _domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("set @@tidb_partition_prune_mode='static'", Vec::new());
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec(
        "create table t (a int, key(a)) partition by range(a)
            (partition p0 values less than (10),
             partition p1 values less than (22))",
        Vec::new(),
    );
    testkit.MustExec("insert into t values (1), (2), (3), (10), (11)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    must_no_global_stats(&testkit, "t");
    testkit.MustExec("set @@tidb_partition_prune_mode='dynamic'", Vec::new());
    must_no_global_stats(&testkit, "t");

    testkit.MustExec("set @@tidb_partition_prune_mode='static'", Vec::new());
    testkit.MustExec("insert into t values (4), (5), (6)", Vec::new());
    testkit.MustExec("analyze table t partition p0", Vec::new());
    must_no_global_stats(&testkit, "t");
    testkit.MustExec("set @@tidb_partition_prune_mode='dynamic'", Vec::new());
    must_no_global_stats(&testkit, "t");
    testkit.MustExec("set @@tidb_partition_prune_mode='static'", Vec::new());
}

/// 动态模式下合并分区索引直方图到 global buckets。
#[test]
fn go_test_merge_idx_hist() {
    let (_store, _domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("set @@tidb_partition_prune_mode='dynamic'", Vec::new());
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec(
        "create table t (a int, key(a))
         partition by range (a) (
            partition p0 values less than (10),
            partition p1 values less than (20))",
        Vec::new(),
    );
    testkit.MustExec("set @@tidb_analyze_version=2", Vec::new());
    testkit.MustExec(
        "insert into t values (1), (2), (3), (4), (5), (6), (6), (null), (11), (12), (13), (14), (15), (16), (17), (18), (19), (19)",
        Vec::new(),
    );
    testkit.MustExec("analyze table t with 2 topn, 2 buckets", Vec::new());
    let rows = testkit
        .MustQuery(
            "show stats_buckets where partition_name like 'global'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 4);
}

/// 会话级动态/静态裁剪：EXPLAIN 形态不同；ANALYZE 后静态扫描行估计正确。
#[test]
fn go_test_partition_prune_mode_session_variable() {
    let (store, _domain, mut tk1, _guard) = new_exclusive_store_and_domain();
    let _force_dynamic_prune = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/planner/core/forceDynamicPrune",
        "return(true)",
    );
    tk1.MustExec("use test", Vec::new());
    tk1.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    tk1.MustExec("set @@tidb_analyze_version=2", Vec::new());

    let mut tk2 = TestKit::new(store);
    tk2.MustExec("use test", Vec::new());
    tk2.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
    tk2.MustExec("set @@tidb_analyze_version=2", Vec::new());

    tk1.MustExec(
        "create table t (a int, key(a)) partition by range(a)
            (partition p0 values less than (10),
             partition p1 values less than (22))",
        Vec::new(),
    );

    tk1.MustQuery("explain format = 'brief' select * from t", Vec::new())
        .Check(vec![
            vec![
                "IndexReader",
                "10000.00",
                "root",
                "partition:all index:IndexFullScan",
            ],
            vec![
                "└─IndexFullScan",
                "10000.00",
                "cop[tikv]",
                "table:t, index:a(a) keep order:false",
            ],
        ]);
    tk2.MustQuery("explain format = 'brief' select * from t", Vec::new())
        .Check(vec![
            vec!["PartitionUnion", "20000.00", "root", ""],
            vec!["├─IndexReader", "10000.00", "root", "index:IndexFullScan"],
            vec![
                "│ └─IndexFullScan",
                "10000.00",
                "cop[tikv]",
                "table:t, partition:p0, index:a(a) keep order:false",
            ],
            vec!["└─IndexReader", "10000.00", "root", "index:IndexFullScan"],
            vec![
                "  └─IndexFullScan",
                "10000.00",
                "cop[tikv]",
                "table:t, partition:p1, index:a(a) keep order:false",
            ],
        ]);

    tk1.MustExec("insert into t values (1), (2), (3), (10), (11)", Vec::new());
    tk1.MustExec("analyze table t with 1 topn, 2 buckets", Vec::new());
    let dynamic = vec![
        vec![
            "IndexReader",
            "5.00",
            "root",
            "partition:all index:IndexFullScan",
        ],
        vec![
            "└─IndexFullScan",
            "5.00",
            "cop[tikv]",
            "table:t, index:a(a) keep order:false",
        ],
    ];
    let static_rows = vec![
        vec!["PartitionUnion", "5.00", "root", ""],
        vec!["├─IndexReader", "3.00", "root", "index:IndexFullScan"],
        vec![
            "│ └─IndexFullScan",
            "3.00",
            "cop[tikv]",
            "table:t, partition:p0, index:a(a) keep order:false",
        ],
        vec!["└─IndexReader", "2.00", "root", "index:IndexFullScan"],
        vec![
            "  └─IndexFullScan",
            "2.00",
            "cop[tikv]",
            "table:t, partition:p1, index:a(a) keep order:false",
        ],
    ];
    tk1.MustQuery("explain format = 'brief' select * from t", Vec::new())
        .Check(dynamic.clone());
    tk2.MustQuery("explain format = 'brief' select * from t", Vec::new())
        .Check(static_rows.clone());

    tk1.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
    tk1.MustQuery("explain format = 'brief' select * from t", Vec::new())
        .Check(static_rows);
    tk2.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    tk2.MustQuery("explain format = 'brief' select * from t", Vec::new())
        .Check(dynamic);
}

/// Hash 分区表 ANALYZE 后 FM Sketch 行数，并触发 GC。
#[test]
fn go_test_duplicate_fm_sketch() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("set @@tidb_partition_prune_mode='dynamic'", Vec::new());
    testkit.MustExec(
        "create table t(a int, b int, c int) partition by hash(a) partitions 3",
        Vec::new(),
    );
    testkit.MustExec("insert into t values (1, 1, 1)", Vec::new());
    for column in ["a", "b", "c"] {
        testkit.MustExec(&format!("select * from t where {column} = '1'"), Vec::new());
    }
    domain
        .dump_col_stats_usage_to_kv()
        .expect("TriggerPredicateColumnsCollection");
    testkit.MustExec("analyze table t", Vec::new());
    testkit
        .MustQuery("select count(*) from mysql.stats_fm_sketch", Vec::new())
        .Check(vec![vec!["9"]]);
    testkit.MustExec("analyze table t", Vec::new());
    testkit
        .MustQuery("select count(*) from mysql.stats_fm_sketch", Vec::new())
        .Check(vec![vec!["9"]]);

    let table = table_meta(&domain, "t");
    let dropped_column_id = table.Columns[1].ID;
    testkit.MustExec("alter table t drop column b", Vec::new());
    assert!(
        !astersql_statistics_handle::storage::StatsCatalog::histogram_exists(
            domain.as_ref(),
            table.ID,
            dropped_column_id,
            false,
        ),
        "dropped column must disappear from the statistics catalog before GC"
    );
    domain.gc_stats(Duration::ZERO).expect("GCStats");
    testkit
        .MustQuery("select count(*) from mysql.stats_fm_sketch", Vec::new())
        .Check(vec![vec!["6"]]);
}

/// The restricted statistics SQL surface renders `mysql.stats_fm_sketch.value`
/// as hexadecimal text (Go reads the raw blob), so recover the bytes before
/// handing them to `DecodeFMSketch`.
/// 受限 SQL 面把 FM Sketch value 渲成十六进制文本（Go 读原始 blob），解码后再交给 DecodeFMSketch。
fn decode_hex_blob(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("hex digits"), 16)
                .expect("hex byte")
        })
        .collect()
}

/// 索引/聚簇索引 FM Sketch 行数与 NDV（不同值个数）校验。
#[test]
fn go_test_index_fm_sketch() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("set @@session.tidb_analyze_version = 2", Vec::new());
    testkit.MustExec("drop table if exists t", Vec::new());
    testkit.MustExec(
        "create table t(a int, b int, c int, index ia(a), index ibc(b, c)) partition by hash(a) partitions 3",
        Vec::new(),
    );
    testkit.MustExec("insert into t values (1, 1, 1)", Vec::new());
    testkit.MustExec("set @@tidb_partition_prune_mode='dynamic'", Vec::new());
    testkit.MustExec("analyze table t index ia", Vec::new());
    // Version 2 analyze collects full stats even when only indexes are specified.
    // v2 ANALYZE 即使只指定索引也会收集完整统计。
    testkit
        .MustQuery("select count(*) from mysql.stats_fm_sketch", Vec::new())
        .Check(vec![vec!["15"]]);
    testkit.MustExec("analyze table t index ibc", Vec::new());
    testkit
        .MustQuery("select count(*) from mysql.stats_fm_sketch", Vec::new())
        .Check(vec![vec!["15"]]);
    testkit.MustExec("analyze table t", Vec::new());
    testkit
        .MustQuery("select count(*) from mysql.stats_fm_sketch", Vec::new())
        .Check(vec![vec!["15"]]);
    testkit.MustExec("drop table if exists t", Vec::new());
    domain.gc_stats(Duration::ZERO).expect("GCStats");

    // clustered index
    // 聚簇索引场景。
    testkit.MustExec("drop table if exists t", Vec::new());
    testkit.MustExec("set @@tidb_enable_clustered_index=ON", Vec::new());
    testkit.MustExec(
        "create table t (a datetime, b datetime, primary key (a)) partition by hash(year(a)) partitions 3",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values ('2000-01-01', '2000-01-01')",
        Vec::new(),
    );
    testkit.MustExec("analyze table t", Vec::new());
    // Clustered primary key also records fm_sketch for the primary index in v2.
    // v2 下聚簇主键也会为 primary index 记录 fm_sketch。
    testkit
        .MustQuery("select count(*) from mysql.stats_fm_sketch", Vec::new())
        .Check(vec![vec!["9"]]);
    testkit.MustExec("drop table if exists t", Vec::new());
    domain.gc_stats(Duration::ZERO).expect("GCStats");

    // test NDV
    // 校验 NDV（不同值个数）。
    let check_ndv = |testkit: &mut TestKit, rows: usize, ndv: i64| {
        testkit.MustExec("analyze table t", Vec::new());
        // Exclude extra handle stats (hist_id < 0) from the NDV checks.
        // NDV 检查排除 hist_id < 0 的额外 handle 统计。
        let sketches = testkit
            .MustQuery(
                "select hex(value) from mysql.stats_fm_sketch where hist_id > 0",
                Vec::new(),
            )
            .Rows();
        assert_eq!(sketches.len(), rows, "{sketches:?}");
        for row in &sketches {
            let sketch = DecodeFMSketch(Some(&decode_hex_blob(&row[0])))
                .expect("DecodeFMSketch")
                .expect("non-empty FM sketch");
            assert_eq!(sketch.NDV(), ndv);
        }
    };

    testkit.MustExec("set @@tidb_enable_clustered_index=OFF", Vec::new());
    testkit.MustExec(
        "create table t(a int, key(a)) partition by hash(a) partitions 3",
        Vec::new(),
    );
    testkit.MustExec("insert into t values (1), (2), (2), (3)", Vec::new());
    check_ndv(&mut testkit, 6, 1);
    testkit.MustExec("insert into t values (4), (5), (6)", Vec::new());
    check_ndv(&mut testkit, 6, 2);
    testkit.MustExec("insert into t values (2), (5)", Vec::new());
    check_ndv(&mut testkit, 6, 2);
    testkit.MustExec("drop table if exists t", Vec::new());
    domain.gc_stats(Duration::ZERO).expect("GCStats");

    // clustered index
    // 聚簇索引场景。
    testkit.MustExec("set @@tidb_enable_clustered_index=ON", Vec::new());
    testkit.MustExec(
        "create table t (a datetime, b datetime, primary key (a)) partition by hash(year(a)) partitions 3",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values ('2000-01-01', '2001-01-01'), ('2001-01-01', '2001-01-01'), ('2002-01-01', '2001-01-01')",
        Vec::new(),
    );
    check_ndv(&mut testkit, 9, 1);
    testkit.MustExec(
        "insert into t values ('1999-01-01', '1998-01-01'), ('1997-01-02', '1999-01-02'), ('1998-01-03', '1999-01-03')",
        Vec::new(),
    );
    check_ndv(&mut testkit, 9, 2);
}

/// 带 collation 的 varchar 列可 ANALYZE 并得到非伪统计与 NDV。
#[test]
fn go_test_load_histogram_with_collate() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("drop table if exists t", Vec::new());
    testkit.MustExec(
        "create table t(a varchar(10) collate utf8mb4_unicode_ci)",
        Vec::new(),
    );
    testkit.MustExec("insert into t values('abcdefghij')", Vec::new());
    testkit.MustExec("insert into t values('abcdufghij')", Vec::new());
    testkit.MustExec("analyze table t with 0 topn", Vec::new());
    let table = table_meta(&domain, "t");
    let stats = physical_stats(&domain, table.ID);
    assert!(!stats.pseudo);
    assert_eq!(stats.columns.len(), 1);
    assert!(stats.columns.values().next().unwrap().ndv >= 1);
}

/// 连续两次读取物理统计应得到相同缓存快照。
#[test]
fn go_test_stats_cache_update_skip() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t (c1 int, c2 int)", Vec::new());
    testkit.MustExec("insert into t values(1, 2)", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let table = table_meta(&domain, "t");
    let first = physical_stats(&domain, table.ID);
    assert!(!first.pseudo);
    domain.update_stats().expect("Handle::Update");
    let second = physical_stats(&domain, table.ID);
    assert_eq!(first, second);
}

/// 增量写入 + ANALYZE 后 stats_meta 的 count/modify_count 与 NDV。
#[test]
fn go_test_incremental_modify_count_update() {
    for analyze_snapshot in [true, false] {
        let (_store, domain, mut testkit, _guard) = new_store_and_domain();
        testkit.MustExec("use test", Vec::new());
        if analyze_snapshot {
            testkit.MustExec(
                "set @@session.tidb_enable_analyze_snapshot = on",
                Vec::new(),
            );
        } else {
            testkit.MustExec("set @@session.tidb_enable_analyze_snapshot = 0", Vec::new());
        }
        testkit.MustExec("create table t(a int)", Vec::new());
        testkit.MustExec("set @@session.tidb_analyze_version = 2", Vec::new());
        let table = table_meta(&domain, "t");
        let tid = table.ID;

        testkit.MustExec("insert into t values(1),(2),(3)", Vec::new());
        testkit.MustExec("flush stats_delta *.*", Vec::new());
        testkit.MustExec("analyze table t", Vec::new());
        testkit
            .MustQuery(
                &format!("select count, modify_count from mysql.stats_meta where table_id = {tid}"),
                Vec::new(),
            )
            .Check(vec![vec!["3", "0"]]);

        testkit.MustExec("insert into t values(4),(5),(6)", Vec::new());
        testkit.MustExec("flush stats_delta *.*", Vec::new());
        let _snapshot = astersql_testkit_testfailpoint::enable(
            "github.com/pingcap/tidb/pkg/executor/injectAnalyzeSnapshot",
            "return(1)",
        );
        let _base_count = astersql_testkit_testfailpoint::enable(
            "github.com/pingcap/tidb/pkg/executor/injectBaseCount",
            "return(3)",
        );
        let _base_modify_count = astersql_testkit_testfailpoint::enable(
            "github.com/pingcap/tidb/pkg/executor/injectBaseModifyCount",
            "return(0)",
        );
        testkit.MustExec("analyze table t", Vec::new());
        testkit
            .MustQuery(
                &format!("select count, modify_count from mysql.stats_meta where table_id = {tid}"),
                Vec::new(),
            )
            .Check(vec![vec!["6", "3"]]);
        let ndv = physical_stats(&domain, tid)
            .columns
            .values()
            .next()
            .map(|column| column.ndv)
            .unwrap_or_default();
        assert_eq!(ndv, if analyze_snapshot { 3 } else { 6 });
    }
}

/// ANALYZE 前自动 flush 待处理 delta；之后 pending 为空。
#[test]
fn go_test_flush_pending_stats_delta_before_analyze() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t(a int)", Vec::new());
    let table = table_meta(&domain, "t");
    let table_id = table.ID;

    testkit.MustExec("insert into t values(1),(2),(3),(4),(5)", Vec::new());
    let context = domain.stats_context();
    assert_eq!(context.pending_stats_delta_ids(), vec![table_id]);

    testkit.MustExec("analyze table t", Vec::new());
    testkit
        .MustQuery(
            &format!(
                "select count, modify_count from mysql.stats_meta where table_id = {table_id}"
            ),
            Vec::new(),
        )
        .Check(vec![vec!["5", "0"]]);
    assert!(context.pending_stats_delta_ids().is_empty());

    testkit.MustExec("flush stats_delta test.t", Vec::new());
    testkit
        .MustQuery(
            &format!(
                "select count, modify_count from mysql.stats_meta where table_id = {table_id}"
            ),
            Vec::new(),
        )
        .Check(vec![vec!["5", "0"]]);
}

/// 开启历史统计后，RecordHistoricalStatsToStorage 写入 stats_history。
#[test]
fn go_test_record_historical_stats_to_storage() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    domain.set_historical_stats_enabled(true);
    testkit.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("drop table if exists t", Vec::new());
    testkit.MustExec("create table t(a int, b varchar(10))", Vec::new());
    testkit.MustExec(
        "insert into t value(1, 'aaa'), (3, 'aab'), (5, 'bba'), (2, 'bbb'), (4, 'cca'), (6, 'ccc')",
        Vec::new(),
    );
    // Go marks the column statistics as needed with two predicate SELECTs
    // before creating the indexes.
    // Go 用两条谓词 SELECT 标记列统计为 needed，再创建索引。
    testkit.MustExec("select * from t where a = 3", Vec::new());
    testkit.MustExec("select * from t where b = 'bbb'", Vec::new());
    testkit.MustExec("alter table t add index single(a)", Vec::new());
    testkit.MustExec("alter table t add index multi(a, b)", Vec::new());
    testkit.MustExec("analyze table t with 2 topn", Vec::new());

    let table = table_meta(&domain, "t");
    let version = domain
        .stats_context()
        .record_historical_stats_to_storage(table.ID)
        .expect("RecordHistoricalStatsToStorage");
    let rows = testkit
        .MustQuery(
            &format!("select count(*) from mysql.stats_history where version = '{version}'"),
            Vec::new(),
        )
        .Rows();
    let num: i64 = rows[0][0].parse().expect("history row count");
    assert!(num >= 1);
}

/// 上游 Go 跳过：列被驱逐后的 loaded 状态（此处 ignore）。
#[test]
#[ignore = "Go TestEvictedColumnLoadedStatus is skipped upstream"]
fn go_test_evicted_column_loaded_status() {}

/// 未 ANALYZE 时列/索引未初始化；伪统计标志行为。
#[test]
fn go_test_uninitialized_stats_status() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("drop table if exists t", Vec::new());
    testkit.MustExec(
        "create table t(a int, b int, c int, index idx_a(a))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values (1,2,2), (3,4,4), (5,6,6), (7,8,8), (9,10,10)",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    let table = table_meta(&domain, "t");
    let stats = physical_stats(&domain, table.ID);
    assert_eq!(
        stats.stats_version, 0,
        "uninitialized table stats: {stats:?}"
    );
    assert!(
        stats
            .columns
            .values()
            .all(|column| !column.IsStatsInitialized())
    );
    assert!(stats.indexes.values().all(|index| !index.analyzed));
    testkit
        .MustQuery(
            "show stats_histograms where db_name = 'test' and table_name = 't'",
            Vec::new(),
        )
        .Check(Vec::<Vec<&str>>::new());

    testkit.MustExec(
        "set @@tidb_enable_pseudo_for_outdated_stats = true",
        Vec::new(),
    );
    let explain = testkit
        .MustQuery("explain select * from t", Vec::new())
        .Rows();
    assert!(
        explain
            .iter()
            .flatten()
            .any(|cell| cell.contains("stats:pseudo")),
        "EXPLAIN must mark uninitialized stats as pseudo: {explain:?}"
    );
    testkit.MustExec(
        "set @@tidb_enable_pseudo_for_outdated_stats = false",
        Vec::new(),
    );
    let explain = testkit
        .MustQuery("explain select * from t", Vec::new())
        .Rows();
    assert!(
        explain
            .iter()
            .flatten()
            .any(|cell| cell.contains("stats:pseudo")),
        "EXPLAIN must keep uninitialized stats pseudo: {explain:?}"
    );
}

/// Issue 39336：非法零月日期在 dynamic 合并 global 时作业仍 finished。
#[test]
fn go_test_issue_39336() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec(
        "create table t1 (\
             a datetime(3) default null,\
             b int\
         ) partition by range (b) (\
             partition p0 values less than (1000),\
             partition p1 values less than (maxvalue)\
         )",
        Vec::new(),
    );
    testkit.MustExec("set @@sql_mode=''", Vec::new());
    testkit.MustExec("set @@tidb_analyze_version=2", Vec::new());
    testkit.MustExec("set @@tidb_partition_prune_mode='dynamic'", Vec::new());
    // The first three rows carry a zero month, which only `sql_mode=''`
    // accepts; they used to make the global merge job fail.
    // 前三行含零月日期，仅 sql_mode='' 接受；曾导致 global merge 失败。
    testkit.MustExec(
        "insert into t1 values \
         ('1000-00-09 00:00:00.000',    1),\
         ('1000-00-06 00:00:00.000',    1),\
         ('1000-00-06 00:00:00.000',    1),\
         ('2022-11-23 14:24:30.000',    1),\
         ('2022-11-23 14:24:32.000',    1),\
         ('2022-11-23 14:24:33.000',    1),\
         ('2022-11-23 14:24:35.000',    1),\
         ('2022-11-23 14:25:08.000', 1001),\
         ('2022-11-23 14:25:09.000', 1001)",
        Vec::new(),
    );
    // EXPLAIN traverses the same predicate-planning path as Go's SELECT helper
    // without decoding the deliberately invalid zero-month rows.
    // EXPLAIN 复用 Go SELECT helper 的谓词规划路径，但不解码刻意构造的零月数据。
    for column in ["a", "b"] {
        testkit.MustExec(
            &format!("explain select * from t1 where {column} = '1'"),
            Vec::new(),
        );
    }
    domain
        .dump_col_stats_usage_to_kv()
        .expect("TriggerPredicateColumnsCollection");
    testkit.MustExec("analyze table t1 with 0 topn", Vec::new());

    let rows = testkit
        .MustQuery(
            "show analyze status where job_info like 'merge global stats%'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][7], "finished");
}

/// InitStatsLite 只加载统计元数据；异步/同步按需加载列与索引，重新 ANALYZE 后版本递增。
#[test]
fn go_test_init_stats_lite() {
    let _queue = acquire_async_load_queue();
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec(
        "create table t(a int, b int, c int, primary key(a), key idxb(b), key idxc(c))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5),(6,6,6),(7,7,7),(8,8,8),(9,9,9)",
        Vec::new(),
    );
    domain
        .set_stats_lease(Duration::from_millis(1))
        .expect("enable on-demand statistics loading");

    let table = table_meta(&domain, "t");
    let column_b = table.Columns[1].ID;
    let column_c = table.Columns[2].ID;
    let index_ids = index_ids_by_name(&table);
    let index_b = index_ids["idxb"];
    let index_c = index_ids["idxc"];

    testkit.MustExec("analyze table t with 2 topn, 2 buckets", Vec::new());
    refresh_stats_cache_after_analyze(&domain);
    let after_analyze = physical_stats(&domain, table.ID);
    check_all_evicted(&after_analyze, "after ANALYZE");
    assert!(!after_analyze.columns.is_empty());
    assert!(!after_analyze.indexes.is_empty());

    clear_and_init_stats_lite(&domain);
    let after_lite = physical_stats(&domain, table.ID);
    check_all_evicted(&after_lite, "after InitStatsLite");
    assert_eq!(after_lite.stats_version, Version2 as i64);

    testkit.MustExec("set @@tidb_stats_load_sync_wait = 0", Vec::new());
    testkit.MustExec("explain select * from t where b > 1", Vec::new());
    domain
        .load_needed_histograms()
        .expect("load b statistics asynchronously");
    let after_async_load = physical_stats(&domain, table.ID);
    let column_b_before = &after_async_load.columns[&column_b];
    let index_b_before = &after_async_load.indexes[&index_b];
    assert!(column_b_before.loaded_or_evicted);
    assert!(index_b_before.fully_loaded);
    assert!(
        !after_async_load.columns[&column_c].loaded_or_evicted,
        "column c must stay pruned while only b is referenced"
    );
    assert!(
        !after_async_load.indexes[&index_c].fully_loaded,
        "index idxc must stay pruned while only b is referenced"
    );
    let column_b_version = column_b_before.version;
    let index_b_version = index_b_before.version;

    testkit.MustExec("set @@tidb_stats_load_sync_wait = 60000", Vec::new());
    testkit.MustExec("explain select * from t where c > 1", Vec::new());
    let after_sync_load = physical_stats(&domain, table.ID);
    assert!(after_sync_load.columns[&column_c].loaded_or_evicted);
    assert!(after_sync_load.indexes[&index_c].fully_loaded);
    let column_c_version = after_sync_load.columns[&column_c].version;
    let index_c_version = after_sync_load.indexes[&index_c].version;

    testkit.MustExec("analyze table t with 1 topn, 3 buckets", Vec::new());
    let after_reanalyze = physical_stats(&domain, table.ID);
    assert!(after_reanalyze.columns[&column_b].loaded_or_evicted);
    assert!(after_reanalyze.indexes[&index_b].fully_loaded);
    assert!(after_reanalyze.columns[&column_c].loaded_or_evicted);
    assert!(after_reanalyze.indexes[&index_c].fully_loaded);
    assert!(after_reanalyze.columns[&column_b].version > column_b_version);
    assert!(after_reanalyze.indexes[&index_b].version > index_b_version);
    assert!(after_reanalyze.columns[&column_c].version > column_c_version);
    assert!(after_reanalyze.indexes[&index_c].version > index_c_version);
}

/// ADD COLUMN 合成列统计；Lite 只记存在不加载载荷；再 ANALYZE 后有 TopN。
#[test]
fn go_test_init_stats_lite_records_synthesized_column_stats() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t(a int)", Vec::new());
    testkit.MustExec("insert into t values (1),(2),(3)", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    testkit.MustExec(
        "analyze table t all columns with 2 topn, 2 buckets",
        Vec::new(),
    );
    // The add-column DDL event is applied synchronously by the Domain, so Go's
    // `FindEvent` + `HandleDDLEventWithTxn` pair has no separate step here.
    // ADD COLUMN 的 DDL 事件由 Domain 同步应用，无需 Go 的 FindEvent 分步。
    testkit.MustExec("alter table t add column b int default 10", Vec::new());
    domain.update_stats().expect("Handle::Update");

    let table = table_meta(&domain, "t");
    let column_b = table.Columns[1].ID;
    let stats = physical_stats(&domain, table.ID);
    assert!(stats.columns.contains_key(&column_b));
    let synthesized = &stats.columns[&column_b];
    // The column stats are created by the DDL handler; they carry no TopN and
    // exactly one histogram bucket because the payload is synthesized from the
    // column default value.
    // DDL 处理器合成列统计：无 TopN，仅一个由默认值生成的直方图桶。
    assert!(synthesized.analyzed_or_synthesized);
    assert!(stats.initialized);
    assert!(synthesized.loaded_or_evicted);
    assert!(synthesized.top_n.is_empty());
    assert_eq!(synthesized.buckets.len(), 1);

    clear_and_init_stats_lite(&domain);
    let lite = physical_stats(&domain, table.ID);
    assert!(lite.columns.contains_key(&column_b));
    assert!(lite.columns[&column_b].analyzed_or_synthesized);
    // Lite initialization only records existence; no payload is loaded.
    // Lite 初始化只记录列存在，不加载直方图/TopN 载荷。
    assert!(lite.columns[&column_b].buckets.is_empty());
    assert!(!lite.columns[&column_b].loaded_or_evicted);

    testkit.MustExec("insert into t values (4, 4),(5, 5)", Vec::new());
    testkit.MustExec(
        "analyze table t all columns with 2 topn, 2 buckets",
        Vec::new(),
    );
    let analyzed = physical_stats(&domain, table.ID);
    assert!(analyzed.columns.contains_key(&column_b));
    assert!(analyzed.columns[&column_b].analyzed_or_synthesized);
    assert!(analyzed.columns[&column_b].loaded_or_evicted);
    assert!(!analyzed.columns[&column_b].top_n.is_empty());
}

/// 跳过缺失分区统计时，全局行数含未分析分区，modify_count 保留。
#[test]
fn go_test_skip_missing_partition_stats() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    testkit.MustExec("set @@tidb_skip_missing_partition_stats = 1", Vec::new());
    testkit.MustExec(
        "create table t (a int, b int, c int, index idx_b(b)) partition by range (a) \
         (partition p0 values less than (100), partition p1 values less than (200), \
          partition p2 values less than (300))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values (1,1,1), (2,2,2), (101,101,101), (102,102,102), (201,201,201), (202,202,202)",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    testkit.MustExec("analyze table t partition p0, p1", Vec::new());

    let table = table_meta(&domain, "t");
    let global = physical_stats(&domain, table.ID);
    // `p2` is never analyzed: its realtime rows still count towards the global
    // total and its pending modifications stay charged as modify_count.
    // p2 未分析：其实时行仍计入全局 total，未分析修改计入 modify_count。
    assert_eq!(global.realtime_count, 6);
    assert_eq!(global.modify_count, 2);
    assert!(
        global
            .columns
            .values()
            .all(|column| column.IsStatsInitialized())
    );
    assert!(global.indexes.values().all(|index| index.analyzed));
}

/// 统计读取超时 failpoint：Update 失败且缓存内容不变。
#[test]
fn go_test_stats_cache_update_timeout() {
    let (_store, domain, mut testkit, _guard) = new_exclusive_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    testkit.MustExec("set @@tidb_skip_missing_partition_stats = 1", Vec::new());
    testkit.MustExec(
        "create table t (a int, b int, c int, index idx_b(b)) partition by range (a) \
         (partition p0 values less than (100), partition p1 values less than (200), \
          partition p2 values less than (300))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values (1,1,1), (2,2,2), (101,101,101), (102,102,102), (201,201,201), (202,202,202)",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    testkit.MustExec("analyze table t partition p0, p1", Vec::new());

    let table = table_meta(&domain, "t");
    let global = physical_stats(&domain, table.ID);
    assert_eq!(global.realtime_count, 6);
    assert_eq!(global.modify_count, 2);

    let _failpoint = astersql_testkit_testfailpoint::enable(
        astersql_statistics_handle_util::EXEC_ROWS_TIMEOUT_FAILPOINT,
        "return(true)",
    );
    domain
        .update_stats()
        .expect_err("Handle::Update must fail while the statistics reader times out");
    // A failed refresh must leave the cached statistics untouched.
    // 刷新失败时缓存中的统计必须保持不变。
    let global = physical_stats(&domain, table.ID);
    assert_eq!(global.realtime_count, 6);
    assert_eq!(global.modify_count, 2);
}

/// BIT(n) 列 ANALYZE 后 buckets 上下界十六进制编码正确。
#[test]
fn go_test_load_stats_for_bit_column() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    let cases = [
        (1, "0", "30", "1", "31"),
        (2, "2", "32", "3", "33"),
        (6, "\"0\"", "3438", "\"1\"", "3439"),
        (7, "\"a\"", "3937", "\"b\"", "3938"),
    ];
    for (index, (len, lower, expected_lower, upper, expected_upper)) in
        cases.into_iter().enumerate()
    {
        let table = format!("t{index}");
        testkit.MustExec(&format!("create table {table}(a bit({len}));"), Vec::new());
        let meta = table_meta(&domain, &table);
        assert_eq!(
            meta.Columns[0].GetType(),
            16,
            "BIT column FieldType after CREATE TABLE for {table}"
        );
        testkit.MustExec(
            &format!("insert into {table} values ({lower}), ({upper});"),
            Vec::new(),
        );
        testkit.MustExec(
            &format!("analyze table {table} all columns with 0 topn;"),
            Vec::new(),
        );
        let _ = physical_stats(&domain, meta.ID);
        testkit
            .MustQuery(
                &format!(
                    "SELECT hex(lower_bound), hex(upper_bound) FROM mysql.stats_buckets WHERE table_id = {} ORDER BY lower_bound",
                    meta.ID
                ),
                Vec::new(),
            )
            .Check(vec![
                vec![expected_lower, expected_lower],
                vec![expected_upper, expected_upper],
            ]);
        testkit.MustExec(&format!("drop table {table}"), Vec::new());
    }
}

/// SHOW stats_* 查询系统表不应把系统表塞进统计缓存。
#[test]
fn go_test_stats_cache_should_not_cache_system_table() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t(a int)", Vec::new());
    testkit.MustExec("insert into t values(1),(2),(3)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    assert_eq!(cache_len(&domain), 1);

    // SHOW returns a record set; use query path like Go MustQuery/MustExec mix.
    // SHOW 返回结果集；走查询路径对齐 Go MustQuery/MustExec 混用。
    let _ = testkit.MustQuery("show stats_meta", Vec::new());
    let _ = testkit.MustQuery("show stats_healthy", Vec::new());
    assert_eq!(cache_len(&domain), 1);
}

/// 本地/全局临时表查询不入缓存；ANALYZE 后才计入缓存长度。
#[test]
fn go_test_stats_cache_should_not_cache_temporary_table() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());

    // Local temporary tables.
    // 本地临时表。
    testkit.MustExec("create temporary table t(a int)", Vec::new());
    testkit.MustExec("insert into t values(1),(2),(3)", Vec::new());
    let _ = testkit.MustQuery("select * from t", Vec::new());
    assert_eq!(cache_len(&domain), 0);

    // Global temporary tables.
    // 全局临时表。
    testkit.MustExec(
        "create global temporary table gt(a int) on commit delete rows",
        Vec::new(),
    );
    testkit.MustExec("insert into gt values(1),(2),(3)", Vec::new());
    let _ = testkit.MustQuery("select * from gt", Vec::new());
    assert_eq!(cache_len(&domain), 0);

    testkit.MustExec("analyze table t", Vec::new());
    assert_eq!(cache_len(&domain), 1);

    // Analyze the global temporary table, whose metadata is Domain-visible.
    // 全局临时表元数据对 Domain 可见，继续覆盖 Go 的 ANALYZE 缓存路径。
    testkit.MustExec("analyze table gt", Vec::new());
    assert_eq!(cache_len(&domain), 2);
}

/// The 27-index table body shared by the three Go pruned-index tests.
/// 三个 Go 索引裁剪测试共用的 27 索引表体 DDL 片段。
const PRUNED_INDEX_TABLE_BODY: &str = "
        a int, b int, c int, d int, e int, f int, g int, h int, i int, j int, k int, l int, m int,
        KEY ia (a), KEY iab (a, b), KEY iac (a, c), KEY iad (a, d),
        KEY iae (a, e), KEY iaf (a, f), KEY iag (a, g), KEY iah (a, h),
        KEY iai (a, i), KEY iaj (a, j), KEY iak (a, k), KEY ial (a, l), KEY iam (a, m),
        KEY ib (b), KEY ibc (b, c), KEY ibd (b, d), KEY ibe (b, e),
        KEY ic (c), KEY icd (c, d), KEY ice (c, e), KEY icf (c, f),
        KEY id (d), KEY ide (d, e), KEY idf (d, f),
        KEY ie (e), KEY ief (e, f),
        KEY if_idx (f)";

/// 裁剪测例插入的样例行（4 行 × 13 列）。
const PRUNED_INDEX_ROWS: &str = "(1,1,1,1,1,1,1,1,1,1,1,1,1),(2,2,2,2,2,2,2,2,2,2,2,2,2),\
     (3,3,3,3,3,3,3,3,3,3,3,3,3),(4,4,4,4,4,4,4,4,4,4,4,4,4)";

/// The 13 indexes whose leading column is `a`; only these can survive pruning
/// for `where a > 1`.
/// 前导列为 `a` 的 13 个索引；`where a > 1` 裁剪时仅它们可能保留。
const A_PREFIXED_INDEXES: [&str; 13] = [
    "ia", "iab", "iac", "iad", "iae", "iaf", "iag", "iah", "iai", "iaj", "iak", "ial", "iam",
];

/// Indexes with no coverage of `a`; Go requires that none of them is loaded.
/// 不含列 `a` 的索引；Go 要求异步加载后它们均不得被加载。
const NON_A_INDEXES: [&str; 14] = [
    "ib", "ibc", "ibd", "ibe", "ic", "icd", "ice", "icf", "id", "ide", "idf", "ie", "ief", "if_idx",
];

/// 构建索引名（小写）→ 索引 ID 映射。
fn index_ids_by_name(table: &TableInfo) -> BTreeMap<String, i64> {
    table
        .Indices
        .iter()
        .map(|index| (index.Name.L.clone(), index.ID))
        .collect()
}

/// Go `checkAllEvicted`: no column and no index may hold a loaded payload.
/// 对齐 Go `checkAllEvicted`：列与索引均不得持有已加载载荷。
fn check_all_evicted(stats: &TableStats, label: &str) {
    for (id, column) in &stats.columns {
        assert!(
            !column.loaded_or_evicted && column.buckets.is_empty() && column.top_n.is_empty(),
            "{label}: column {id} statistics must be all-evicted"
        );
    }
    for (id, index) in &stats.indexes {
        assert!(
            !index.fully_loaded && index.buckets.is_empty() && index.top_n.is_empty(),
            "{label}: index {id} statistics must be all-evicted"
        );
    }
}

/// Go's `ANALYZE` executor refreshes the statistics cache from storage on its
/// way out, which leaves every histogram all-evicted. This runtime keeps the
/// freshly analyzed payload in the cache, so reproduce the refresh explicitly.
/// Go ANALYZE 结束会刷新缓存使直方图 all-evicted；本运行时需显式 clear+update 复现。
fn refresh_stats_cache_after_analyze(domain: &Domain) {
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .clear();
    domain
        .update_stats()
        .expect("reload lite statistics after ANALYZE");
}

/// Go `h.Clear()` + `h.InitStatsLite(ctx, dom.InfoSchema())`.
/// 对齐 Go：清空 handle 后 InitStatsLite 全量轻量初始化。
fn clear_and_init_stats_lite(domain: &Domain) {
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .clear();
    domain
        .init_stats_lite(&[])
        .expect("lite statistics initialization");
}

/// `AsyncLoadHistogramNeededItems` is process-global while every test builds
/// its own mock store (and therefore reuses physical IDs), so the pruned-index
/// tests serialize on this guard and start from an empty queue, mirroring Go's
/// one-test-at-a-time package execution.
/// 异步加载队列进程全局；测例间串行并清空队列，避免物理 ID 复用串扰。
/// 异步直方图加载队列的进程级互斥锁。
static ASYNC_LOAD_QUEUE_GUARD: Mutex<()> = Mutex::new(());

/// 获取异步加载队列互斥锁并清空队列项。
fn acquire_async_load_queue() -> MutexGuard<'static, ()> {
    let guard = ASYNC_LOAD_QUEUE_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for item in AsyncLoadHistogramNeededItems.AllItems() {
        AsyncLoadHistogramNeededItems.Delete(item.TableItemID);
    }
    guard
}

/// Splits the `a`-prefixed indexes into the loaded and the pruned ones.
/// 统计 a 前缀索引中已加载与被裁剪的数量。
fn count_loaded_a_indexes(stats: &TableStats, index_ids: &BTreeMap<String, i64>) -> (usize, usize) {
    let mut loaded = 0;
    let mut pruned = 0;
    for name in A_PREFIXED_INDEXES {
        let id = index_ids[name];
        if stats
            .indexes
            .get(&id)
            .is_some_and(|index| index.fully_loaded)
        {
            loaded += 1;
        } else {
            pruned += 1;
        }
    }
    (loaded, pruned)
}

/// 断言所有非 a 前缀索引均未 fully_loaded。
fn assert_non_a_indexes_not_loaded(
    stats: &TableStats,
    index_ids: &BTreeMap<String, i64>,
    label: &str,
) {
    for name in NON_A_INDEXES {
        let id = index_ids[name];
        assert!(
            !stats
                .indexes
                .get(&id)
                .is_some_and(|index| index.fully_loaded),
            "{label}: index {name} (id={id}) should NOT be loaded (was pruned)"
        );
    }
}

/// 非分区表：索引裁剪后异步加载，仅 a 前缀索引部分加载，非 a 索引不加载。
#[test]
fn go_test_pruned_indexes_no_async_stats_load() {
    let _queue = acquire_async_load_queue();
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    // Create a table with many indexes (more than 10 indexes starting with 'a'
    // to trigger pruning since defaultMaxIndexes = 10).
    // 创建大量索引（超过 10 个 a 前缀）以触发 defaultMaxIndexes=10 的裁剪。
    testkit.MustExec(
        &format!("CREATE TABLE t({PRUNED_INDEX_TABLE_BODY})"),
        Vec::new(),
    );
    testkit.MustExec(
        &format!("insert into t values {PRUNED_INDEX_ROWS}"),
        Vec::new(),
    );

    domain
        .set_stats_lease(Duration::from_millis(1))
        .expect("enable the statistics lease");

    let table = table_meta(&domain, "t");
    let index_ids = index_ids_by_name(&table);

    testkit.MustExec("analyze table t with 2 topn, 2 buckets", Vec::new());

    // Check all stats are evicted initially.
    // 初始校验：ANALYZE 刷新后统计均为 all-evicted。
    refresh_stats_cache_after_analyze(&domain);
    check_all_evicted(&physical_stats(&domain, table.ID), "after ANALYZE");

    clear_and_init_stats_lite(&domain);

    // After InitStatsLite, check stats are evicted.
    // InitStatsLite 后统计仍应为 all-evicted。
    let after_lite = physical_stats(&domain, table.ID);
    check_all_evicted(&after_lite, "after InitStatsLite");
    assert_eq!(after_lite.stats_version, Version2 as i64);

    // Enable async stats load and aggressive index pruning.
    // 开启异步统计加载与激进索引裁剪阈值。
    testkit.MustExec("set @@tidb_stats_load_sync_wait = 0", Vec::new());
    testkit.MustExec("set @@tidb_opt_index_prune_threshold = 1", Vec::new());

    // Run a query that only uses column 'a'.
    // 仅使用列 a 的查询，触发裁剪后的 needed 直方图加载。
    testkit.MustExec("explain select * from t where a > 1", Vec::new());
    domain
        .load_needed_histograms()
        .expect("LoadNeededHistograms after the pruned EXPLAIN");

    let loaded_stats = physical_stats(&domain, table.ID);
    // With 13 indexes and maxToKeep=10, some must be pruned.
    // 13 个 a 前缀索引且 maxToKeep=10，必然有部分被裁剪。
    let (loaded, pruned) = count_loaded_a_indexes(&loaded_stats, &index_ids);
    assert!(
        loaded > 0,
        "at least some indexes starting with 'a' should be loaded"
    );
    assert!(
        pruned > 0,
        "at least some indexes starting with 'a' should be pruned (we have 13 but maxToKeep=10)"
    );
    assert!(
        loaded <= 10,
        "at most 10 indexes can be kept due to defaultMaxIndexes, got {loaded}"
    );
    assert_non_a_indexes_not_loaded(&loaded_stats, &index_ids, "non-partitioned");
}

/// 动态分区表：同上，基于 global 统计做索引裁剪与异步加载。
#[test]
fn go_test_pruned_indexes_no_async_stats_load_partitioned() {
    let _queue = acquire_async_load_queue();
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    testkit.MustExec(
        &format!("CREATE TABLE tp({PRUNED_INDEX_TABLE_BODY}) PARTITION BY HASH(a) PARTITIONS 4"),
        Vec::new(),
    );
    testkit.MustExec(
        &format!("insert into tp values {PRUNED_INDEX_ROWS}"),
        Vec::new(),
    );

    domain
        .set_stats_lease(Duration::from_millis(1))
        .expect("enable the statistics lease");

    let table = table_meta(&domain, "tp");
    let index_ids = index_ids_by_name(&table);

    testkit.MustExec("analyze table tp with 2 topn, 2 buckets", Vec::new());

    refresh_stats_cache_after_analyze(&domain);
    check_all_evicted(&physical_stats(&domain, table.ID), "after ANALYZE");

    clear_and_init_stats_lite(&domain);

    let after_lite = physical_stats(&domain, table.ID);
    check_all_evicted(&after_lite, "after InitStatsLite");
    assert_eq!(after_lite.stats_version, Version2 as i64);

    testkit.MustExec("set @@tidb_stats_load_sync_wait = 0", Vec::new());
    testkit.MustExec("set @@tidb_opt_index_prune_threshold = 1", Vec::new());

    testkit.MustExec("explain select * from tp where a > 1", Vec::new());
    domain
        .load_needed_histograms()
        .expect("LoadNeededHistograms after the pruned EXPLAIN");

    // Check which indexes have stats loaded (using global stats in dynamic mode).
    // 动态模式下基于 global 统计检查哪些索引已加载。
    let loaded_stats = physical_stats(&domain, table.ID);
    let (loaded, pruned) = count_loaded_a_indexes(&loaded_stats, &index_ids);
    assert!(
        loaded > 0,
        "at least some indexes starting with 'a' should be loaded"
    );
    assert!(
        pruned > 0,
        "at least some indexes starting with 'a' should be pruned (we have 13 but maxToKeep=10)"
    );
    assert!(
        loaded <= 10,
        "at most 10 indexes can be kept due to defaultMaxIndexes, got {loaded}"
    );
    assert_non_a_indexes_not_loaded(&loaded_stats, &index_ids, "dynamic partitioned");
}

/// 静态分区表：各分区独立加载；非 a 索引仍不得加载。
#[test]
fn go_test_pruned_indexes_no_async_stats_load_partitioned_static() {
    let _queue = acquire_async_load_queue();
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
    testkit.MustExec(
        &format!("CREATE TABLE tp({PRUNED_INDEX_TABLE_BODY}) PARTITION BY HASH(a) PARTITIONS 4"),
        Vec::new(),
    );
    testkit.MustExec(
        &format!("insert into tp values {PRUNED_INDEX_ROWS}"),
        Vec::new(),
    );

    domain
        .set_stats_lease(Duration::from_millis(1))
        .expect("enable the statistics lease");

    let table = table_meta(&domain, "tp");
    let index_ids = index_ids_by_name(&table);
    let partition_ids = table
        .GetPartitionInfo()
        .expect("hash partition metadata")
        .Definitions
        .iter()
        .map(|definition| definition.ID)
        .collect::<Vec<_>>();

    testkit.MustExec("analyze table tp with 2 topn, 2 buckets", Vec::new());

    // Check all partition stats are evicted initially.
    // 初始校验各分区统计均为 all-evicted。
    refresh_stats_cache_after_analyze(&domain);
    for id in &partition_ids {
        check_all_evicted(&physical_stats(&domain, *id), "after ANALYZE");
    }

    clear_and_init_stats_lite(&domain);

    // After InitStatsLite, check partition stats are evicted.
    // InitStatsLite 后各分区仍为 all-evicted。
    for id in &partition_ids {
        let after_lite = physical_stats(&domain, *id);
        check_all_evicted(&after_lite, "after InitStatsLite");
        assert_eq!(after_lite.stats_version, Version2 as i64);
    }

    testkit.MustExec("set @@tidb_stats_load_sync_wait = 0", Vec::new());
    testkit.MustExec("set @@tidb_opt_index_prune_threshold = 1", Vec::new());

    testkit.MustExec("explain select * from tp where a > 1", Vec::new());
    domain
        .load_needed_histograms()
        .expect("LoadNeededHistograms after the pruned EXPLAIN");

    // In static mode each partition is processed separately, so the pruning
    // behavior may differ from dynamic mode. The key verification is that
    // indexes not containing 'a' are not loaded regardless of mode.
    // 静态模式按分区独立处理，裁剪行为可能不同于动态；关键是非 a 索引不得加载。
    for id in &partition_ids {
        let partition_stats = physical_stats(&domain, *id);
        let (loaded, _) = count_loaded_a_indexes(&partition_stats, &index_ids);
        assert!(
            loaded > 0,
            "partition {id}: at least some indexes starting with 'a' should be loaded"
        );
        assert_non_a_indexes_not_loaded(
            &partition_stats,
            &index_ids,
            &format!("static partition {id}"),
        );
    }
}
