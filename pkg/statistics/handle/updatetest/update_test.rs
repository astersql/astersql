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

// 统计增量更新（stats delta / update）与 Go `update_test.go` 同路径集成测试。
//
// 覆盖单/多会话插入删除、事务回滚、分区表、自动分析（auto analyze）、
// 谓词列追踪、统计锁、TopN 合并与直方图区间切分等场景，校验刷盘后的
// `realtime_count` / `modify_count` 与相关元数据。

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use astersql_statistics::{
    EffectiveAutoAnalyzeMinCnt, HistogramRange, MergeTopN, NewHistogram, NewTopN,
    ResetAutoAnalyzeMinCnt, SetAutoAnalyzeMinCnt, TopNMeta,
};
use astersql_statistics_handle::{TableStats, dump_stats_delta_ratio, set_dump_stats_delta_ratio};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::AnalyzeStatsStore;
use astersql_types::datum::{NewIntDatum, NewStringDatum};
use astersql_types::field::FieldType;

use crate::main_test::new_mock_store_and_domain;

/// 按物理表/分区 ID 从 Domain 统计句柄取出当前 TableStats 快照。
fn physical_stats(store: &AnalyzeStatsStore, physical_id: i64) -> TableStats {
    store
        .domain()
        .stats_handle()
        .lock()
        .expect("domain statistics handle")
        .stats_meta(physical_id)
        .cloned()
        .expect("physical statistics")
}

/// 解析 `test.<table>` 在统计目录中的物理 table_id。
fn table_id(store: &AnalyzeStatsStore, table: &str) -> i64 {
    store
        .domain()
        .stats_table("test", table)
        .unwrap_or_else(|| panic!("missing table test.{table}"))
        .0
        .table_id
}

/// 临时覆盖 dump stats delta 比例阈值；析构时恢复，并用进程锁串行化并发用例。
struct DumpStatsDeltaRatioGuard {
    _lock: MutexGuard<'static, ()>,
    previous: f64,
}

impl DumpStatsDeltaRatioGuard {
    /// 设置新的 dump 比例并保存旧值以便 Drop 恢复。
    fn set(ratio: f64) -> Self {
        static LOCK: Mutex<()> = Mutex::new(());
        let lock = LOCK.lock().expect("dump stats delta ratio lock poisoned");
        let previous = dump_stats_delta_ratio();
        set_dump_stats_delta_ratio(ratio);
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for DumpStatsDeltaRatioGuard {
    fn drop(&mut self) {
        set_dump_stats_delta_ratio(self.previous);
    }
}

/// 临时覆盖自动分析最小行数阈值；析构时 `ResetAutoAnalyzeMinCnt`。
struct AutoAnalyzeMinCntGuard {
    _lock: MutexGuard<'static, ()>,
}

impl AutoAnalyzeMinCntGuard {
    /// 设置 `AutoAnalyzeMinCnt` 并持有全局锁，避免用例互相踩踏。
    fn set(value: i64) -> Self {
        static LOCK: Mutex<()> = Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        SetAutoAnalyzeMinCnt(value);
        Self { _lock: lock }
    }
}

impl Drop for AutoAnalyzeMinCntGuard {
    fn drop(&mut self) {
        ResetAutoAnalyzeMinCnt();
    }
}

/// Go `TestSingleSessionInsert`.
/// 单会话插入/删除/更新与事务提交后，强制 flush 校验 realtime_count 与 modify_count。
#[test]
fn go_test_single_session_insert() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec("set @@session.tidb_analyze_version = 2", Vec::new());
    // PK equality deletes stand in for Go `DELETE ... LIMIT 1` (full executor gap).
    testkit.MustExec("create table t1 (c1 int primary key, c2 int)", Vec::new());
    testkit.MustExec("create table t2 (c1 int primary key, c2 int)", Vec::new());

    let row_count1 = 10;
    let row_count2 = 20;
    for i in 0..row_count1 {
        testkit.MustExec(&format!("insert into t1 values({i}, 0)"), Vec::new());
    }
    for i in 0..row_count2 {
        testkit.MustExec(&format!("insert into t2 values({i}, -1)"), Vec::new());
    }

    let key1_id = table_id(&store, "t1");
    let key2_id = table_id(&store, "t2");
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(physical_stats(&store, key1_id).realtime_count, row_count1);
    assert_eq!(physical_stats(&store, key2_id).realtime_count, row_count2);

    testkit.MustExec("analyze table t1", Vec::new());
    for i in row_count1..(row_count1 * 2) {
        testkit.MustExec(&format!("insert into t1 values({i}, 2)"), Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(
        physical_stats(&store, key1_id).realtime_count,
        row_count1 * 2
    );

    testkit.MustExec("begin", Vec::new());
    for i in (row_count1 * 2)..(row_count1 * 3) {
        testkit.MustExec(&format!("insert into t1 values({i}, 2)"), Vec::new());
    }
    testkit.MustExec("commit", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(
        physical_stats(&store, key1_id).realtime_count,
        row_count1 * 3
    );

    testkit.MustExec("begin", Vec::new());
    for i in (row_count1 * 3)..(row_count1 * 4) {
        testkit.MustExec(&format!("insert into t1 values({i}, 2)"), Vec::new());
    }
    for i in 0..row_count1 {
        testkit.MustExec(&format!("delete from t1 where c1 = {i}"), Vec::new());
    }
    for i in 0..row_count2 {
        testkit.MustExec(&format!("update t2 set c2 = c1 where c1 = {i}"), Vec::new());
    }
    testkit.MustExec("commit", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(
        physical_stats(&store, key1_id).realtime_count,
        row_count1 * 3
    );
    assert_eq!(physical_stats(&store, key2_id).realtime_count, row_count2);

    testkit.MustExec("begin", Vec::new());
    testkit.MustExec("delete from t1", Vec::new());
    testkit.MustExec("commit", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(physical_stats(&store, key1_id).realtime_count, 0);

    let mut modify_counts = testkit
        .MustQuery("select modify_count from mysql.stats_meta", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row[0].parse::<i64>().unwrap())
        .collect::<Vec<_>>();
    modify_counts.sort_unstable();
    assert_eq!(modify_counts, vec![40, 70]);

    let mut total_column_sizes = testkit
        .MustQuery(
            "select tot_col_size from mysql.stats_histograms",
            Vec::new(),
        )
        .Rows()
        .into_iter()
        .map(|row| row[0].parse::<i64>().unwrap())
        .collect::<Vec<_>>();
    total_column_sizes.sort_unstable();
    assert_eq!(total_column_sizes, vec![0, 0, 10, 10]);

    let _ratio = DumpStatsDeltaRatioGuard::set(0.5);
    for i in 0..row_count1 {
        testkit.MustExec(&format!("insert into t1 values ({i},2)"), Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(physical_stats(&store, key1_id).realtime_count, row_count1);

    testkit.MustExec(
        &format!("insert into t1 values ({},2)", row_count1),
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    // Forced `*.*` flush still dumps; ratio only gates non-forced DumpStatsDeltaToKV(false).
    assert_eq!(
        physical_stats(&store, key1_id).realtime_count,
        row_count1 + 1
    );

    domain.flush_stats().expect("FlushStats");
    assert_eq!(
        physical_stats(&store, key1_id).realtime_count,
        row_count1 + 1
    );
}

/// Go `TestRollback`.
/// 事务回滚后增量不应计入统计：realtime_count 与 modify_count 均为 0。
#[test]
fn go_test_rollback() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    testkit.MustExec("create table t (a int, b int)", Vec::new());
    testkit.MustExec("begin", Vec::new());
    testkit.MustExec("insert into t values (1,2)", Vec::new());
    testkit.MustExec("rollback", Vec::new());
    let key_id = table_id(&store, "t");
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    let stats = physical_stats(&store, key_id);
    assert_eq!(stats.realtime_count, 0);
    assert_eq!(stats.modify_count, 0);
}

/// Go `TestMultiSession`.
/// 多 TestKit 会话并发插入/删除后关闭部分会话，flush 仍汇总正确行数。
#[test]
fn go_test_multi_session() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    // PK equality deletes stand in for Go `DELETE ... LIMIT 1`.
    testkit.MustExec("create table t1 (c1 int primary key, c2 int)", Vec::new());
    let row_count1 = 10;
    for i in 0..row_count1 {
        testkit.MustExec(&format!("insert into t1 values({i}, 2)"), Vec::new());
    }

    let mut testkit1 = TestKit::new(store.clone());
    for i in row_count1..(row_count1 * 2) {
        testkit1.MustExec(&format!("insert into test.t1 values({i}, 2)"), Vec::new());
    }
    let mut testkit2 = TestKit::new(store.clone());
    for i in 0..row_count1 {
        testkit2.MustExec(&format!("delete from test.t1 where c1 = {i}"), Vec::new());
    }

    let key1_id = table_id(&store, "t1");
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(physical_stats(&store, key1_id).realtime_count, row_count1);

    for i in (row_count1 * 2)..(row_count1 * 3) {
        testkit.MustExec(&format!("insert into t1 values({i}, 2)"), Vec::new());
    }
    for i in (row_count1 * 3)..(row_count1 * 4) {
        testkit1.MustExec(&format!("insert into test.t1 values({i}, 2)"), Vec::new());
    }
    for i in row_count1..(row_count1 * 2) {
        testkit2.MustExec(&format!("delete from test.t1 where c1 = {i}"), Vec::new());
    }

    testkit.Session().close().expect("close primary session");
    testkit2.Session().close().expect("close second session");

    testkit1.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(
        physical_stats(&store, key1_id).realtime_count,
        row_count1 * 2
    );

    let testkit = TestKit::new(store.clone());
    let rows = testkit
        .MustQuery("select modify_count from mysql.stats_meta", Vec::new())
        .Rows();
    assert_eq!(rows, vec![vec!["60".to_owned()]]);
}

/// Go `TestTxnWithFailure`.
/// 未提交事务内 dump 不计增量；提交后生效；主键冲突失败不影响已提交计数。
#[test]
fn go_test_txn_with_failure() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec("create table t1 (c1 int primary key, c2 int)", Vec::new());
    let key1_id = table_id(&store, "t1");

    let row_count1 = 10;
    testkit.MustExec("begin", Vec::new());
    for i in 0..row_count1 {
        testkit.MustExec(&format!("insert into t1 values({i}, 2)"), Vec::new());
    }
    domain
        .dump_stats_delta_to_kv(true)
        .expect("DumpStatsDeltaToKV");
    assert_eq!(physical_stats(&store, key1_id).realtime_count, 0);
    testkit.MustExec("commit", Vec::new());

    domain
        .dump_stats_delta_to_kv(true)
        .expect("DumpStatsDeltaToKV");
    assert_eq!(physical_stats(&store, key1_id).realtime_count, row_count1);

    let error = testkit
        .Exec("insert into t1 values(0, 2)", Vec::new())
        .unwrap_err();
    assert!(error.message().to_ascii_lowercase().contains("duplicate"));

    domain
        .dump_stats_delta_to_kv(true)
        .expect("DumpStatsDeltaToKV");
    assert_eq!(physical_stats(&store, key1_id).realtime_count, row_count1);

    testkit.MustExec("insert into t1 values(-1, 2)", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(
        physical_stats(&store, key1_id).realtime_count,
        row_count1 + 1
    );
}

/// Go `TestUpdatePartition`.
/// 静态剪枝分区表：各分区物理统计在 insert/update/delete 后独立更新 modify/realtime。
#[test]
fn go_test_update_partition() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let _domain = store.domain();
    testkit.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
    testkit.MustExec("drop table if exists t", Vec::new());
    testkit.MustExec(
        "CREATE TABLE t (a int, b char(5)) PARTITION BY RANGE (a) \
         (PARTITION p0 VALUES LESS THAN (6),PARTITION p1 VALUES LESS THAN (11))",
        Vec::new(),
    );
    let table_info = store.domain().stats_table("test", "t").unwrap().1;
    let partitions = table_info.GetPartitionInfo().expect("partition info");
    assert_eq!(partitions.Definitions.len(), 2);
    let b_col_id = table_info.Columns[1].ID;

    testkit.MustExec(r#"insert into t values (1, "a"), (7, "a")"#, Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    for definition in &partitions.Definitions {
        let stats = physical_stats(&store, definition.ID);
        assert_eq!(stats.modify_count, 1);
        assert_eq!(stats.realtime_count, 1);
        assert_eq!(
            stats
                .columns
                .get(&b_col_id)
                .map(|column| column.total_column_size)
                .unwrap_or(0),
            0
        );
    }

    testkit.MustExec(r#"update t set a = a + 1, b = "aa""#, Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    for definition in &partitions.Definitions {
        let stats = physical_stats(&store, definition.ID);
        assert_eq!(stats.modify_count, 2);
        assert_eq!(stats.realtime_count, 1);
        assert_eq!(
            stats
                .columns
                .get(&b_col_id)
                .map(|column| column.total_column_size)
                .unwrap_or(0),
            0
        );
    }

    testkit.MustExec("delete from t", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    for definition in &partitions.Definitions {
        let stats = physical_stats(&store, definition.ID);
        assert_eq!(stats.modify_count, 3);
        assert_eq!(stats.realtime_count, 0);
        assert_eq!(
            stats
                .columns
                .get(&b_col_id)
                .map(|column| column.total_column_size)
                .unwrap_or(0),
            0
        );
    }
}

/// Go `TestAutoUpdate` (core delta + HandleAutoAnalyze path; lease/index histogram load deferred).
/// 按修改比例触发/跳过自动分析：成功后 modify_count 清零，跳过则保留增量。
#[test]
fn go_test_auto_update() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    let _min = AutoAnalyzeMinCntGuard::set(0);
    testkit.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
    testkit.MustExec("create table t (a varchar(20))", Vec::new());
    testkit.MustExec("select * from t where a > 'a'", Vec::new());
    domain
        .set_auto_analyze_ratio(0.2)
        .expect("set auto analyze ratio");

    let key_id = table_id(&store, "t");
    assert_eq!(physical_stats(&store, key_id).realtime_count, 0);

    testkit.MustExec(
        "insert into t values ('ss'), ('ss'), ('ss'), ('ss'), ('ss')",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    domain.try_handle_auto_analyze().expect("HandleAutoAnalyze");
    let stats = physical_stats(&store, key_id);
    assert_eq!(stats.realtime_count, 5);
    assert_eq!(stats.modify_count, 0);
    assert_eq!(stats.columns.len(), 1);
    assert_eq!(stats.columns.values().next().unwrap().total_column_size, 15);

    domain
        .set_stats_lease(std::time::Duration::from_secs(1))
        .expect("set statistics lease");
    testkit.MustExec("insert into t values ('fff')", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    // 1/6 < 0.2 => skip
    assert!(!domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    let stats = physical_stats(&store, key_id);
    assert_eq!(stats.realtime_count, 6);
    assert_eq!(stats.modify_count, 1);

    testkit.MustExec("insert into t values ('fff')", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    // 2/7 > 0.2 => analyze
    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    let stats = physical_stats(&store, key_id);
    assert_eq!(stats.realtime_count, 7);
    assert_eq!(stats.modify_count, 0);

    testkit.MustExec("insert into t values ('eee')", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    // 1/8 < 0.2 => skip
    assert!(!domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    let stats = physical_stats(&store, key_id);
    assert_eq!(stats.realtime_count, 8);
    assert_eq!(stats.modify_count, 1);
    assert_eq!(stats.columns.len(), 1);
    assert_eq!(stats.columns.values().next().unwrap().total_column_size, 23);

    testkit.MustExec("analyze table t", Vec::new());
    testkit.MustExec("create index idx on t(a)", Vec::new());
    let info = store.domain().stats_table("test", "t").unwrap().1;
    let index_id = info.Indices[0].ID;
    assert!(
        !physical_stats(&store, info.ID)
            .indexes
            .contains_key(&index_id)
    );
    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    let stats = physical_stats(&store, info.ID);
    assert_eq!(stats.realtime_count, 8);
    assert_eq!(stats.modify_count, 0);
    let index = stats
        .indexes
        .get(&index_id)
        .expect("auto analyze loads the new index statistics");
    assert_eq!(index.ndv, 3);
    assert!(index.buckets.is_empty());
    assert_eq!(index.top_n.len(), 3);
    domain
        .set_stats_lease(std::time::Duration::ZERO)
        .expect("restore statistics lease");
}

/// Go `TestAutoUpdatePartition`.
/// 分区表在达到 auto-analyze 比例后，对分区物理 ID 执行分析并清零 modify_count。
#[test]
fn go_test_auto_update_partition() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    let _min = AutoAnalyzeMinCntGuard::set(0);
    testkit.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
    testkit.MustExec("drop table if exists t", Vec::new());
    testkit.MustExec(
        "create table t (a int, index idx(a)) PARTITION BY RANGE (a) (PARTITION p0 VALUES LESS THAN (6))",
        Vec::new(),
    );
    testkit.MustExec("analyze table t", Vec::new());
    domain
        .set_auto_analyze_ratio(0.6)
        .expect("set auto analyze ratio");

    let info = store.domain().stats_table("test", "t").unwrap().1;
    let partition_id = info.GetPartitionInfo().unwrap().Definitions[0].ID;
    assert_eq!(physical_stats(&store, partition_id).realtime_count, 0);

    testkit.MustExec("insert into t values (1)", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    let stats = physical_stats(&store, partition_id);
    assert_eq!(stats.realtime_count, 1);
    assert_eq!(stats.modify_count, 0);
}

/// Go `TestIssue25700`: auto-analyze must handle a generated-column table
/// after a post-analyze delta.
/// 生成列（generated column）表在 analyze 后再有增量时，自动分析仍应成功。
#[test]
fn go_test_issue_25700() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec(
        "set global tidb_auto_analyze_start_time='00:00 +0000'",
        Vec::new(),
    );
    testkit.MustExec(
        "set global tidb_auto_analyze_end_time='23:59 +0000'",
        Vec::new(),
    );
    testkit.MustExec(
        "create table t (\
            ldecimal decimal(32,4) default null, \
            rdecimal decimal(32,4) default null, \
            gen_col decimal(36,4) generated always as (ldecimal + rdecimal) virtual, \
            col_timestamp timestamp(3) null default null\
        )",
        Vec::new(),
    );
    let id = table_id(&store, "t");
    testkit.MustExec("analyze table t", Vec::new());
    let values = std::iter::repeat_n(
        "(2265.2200, 9843.4100, '1999-12-31 16:00:00')",
        (EffectiveAutoAnalyzeMinCnt() + 1) as usize,
    )
    .collect::<Vec<_>>()
    .join(", ");
    testkit.MustExec(
        &format!("insert into t (ldecimal, rdecimal, col_timestamp) values {values}"),
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta *.*", Vec::new());

    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    let stats = physical_stats(&store, id);
    assert_eq!(stats.realtime_count, EffectiveAutoAnalyzeMinCnt() + 1);
    assert_eq!(stats.modify_count, 0);
    assert!(stats.last_analyze_version > 0);
    let status = testkit.MustQuery("show analyze status", Vec::new()).Rows();
    assert!(status.len() > 1);
    assert_eq!(status[1][7], "finished");
}

/// Go `TestLoadHistCorrelation`.
/// 清空缓存后 Update，再经 explain 触发 needed histogram 加载，校验列/索引统计存在。
#[test]
fn go_test_load_hist_correlation() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    domain
        .set_stats_lease(std::time::Duration::from_secs(1))
        .expect("set statistics lease");
    testkit.MustExec("create table t(c int, index idx(c))", Vec::new());
    testkit.MustExec("insert into t values(1),(2),(3),(4),(5)", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());

    domain.stats_handle().lock().expect("stats handle").clear();
    domain.update_stats().expect("update statistics cache");
    let lite_stats = physical_stats(&store, table_id(&store, "t"));
    let column = lite_stats.columns.values().next().unwrap();
    let index = lite_stats.indexes.values().next().unwrap();
    assert!(column.analyzed_or_synthesized);
    assert!(!column.loaded_or_evicted);
    assert!(index.analyzed);
    assert!(!index.fully_loaded);
    assert!(
        testkit
            .MustQuery("show stats_histograms where Table_name = 't'", Vec::new(),)
            .Rows()
            .is_empty()
    );

    testkit.MustQuery("explain select * from t where c = 1", Vec::new());
    domain
        .load_needed_histograms()
        .expect("load needed histograms");
    let stats = physical_stats(&store, table_id(&store, "t"));
    assert_eq!(stats.columns.len(), 1);
    assert_eq!(stats.indexes.len(), 1);
    assert_eq!(stats.columns.values().next().unwrap().correlation, 1.0);
    let histograms = testkit
        .MustQuery("show stats_histograms where Table_name = 't'", Vec::new())
        .Rows();
    assert_eq!(histograms.len(), 2);
    assert_eq!(histograms[0][9], "1");
}

/// Go `TestStatsVariables`.
/// 全局会话变量写入后，Domain 统计侧能读到 analyze_version / prune_mode 等配置。
#[test]
fn go_test_stats_variables() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();

    let variables = domain.stats_session_vars();
    assert_eq!(variables.analyze_version, 2);
    assert_eq!(variables.partition_prune_mode, "dynamic");
    assert!(!variables.historical_stats);
    assert!(!variables.analyze_snapshot);
    assert!(variables.skip_missing_partition_stats);
    assert_eq!(domain.get_current_prune_mode(), "dynamic");

    testkit.MustExec("set global tidb_analyze_version = 2", Vec::new());
    testkit.MustExec(
        "set global tidb_partition_prune_mode = 'static'",
        Vec::new(),
    );
    testkit.MustExec("set global tidb_enable_historical_stats = on", Vec::new());
    testkit.MustExec("set global tidb_enable_analyze_snapshot = 1", Vec::new());
    testkit.MustExec(
        "set global tidb_skip_missing_partition_stats = false",
        Vec::new(),
    );

    let variables = domain.stats_session_vars();
    assert_eq!(variables.analyze_version, 2);
    assert_eq!(variables.partition_prune_mode, "static");
    assert!(variables.historical_stats);
    assert!(variables.analyze_snapshot);
    assert!(!variables.skip_missing_partition_stats);
    assert_eq!(domain.get_current_prune_mode(), "static");
}

/// Go `TestAutoUpdatePartitionInDynamicOnlyMode`.
/// 动态剪枝模式下全局与分区物理统计同步增量，自动分析同时清零两侧 modify_count。
#[test]
fn go_test_auto_update_partition_in_dynamic_only_mode() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    let _min = AutoAnalyzeMinCntGuard::set(0);
    testkit.MustExec(
        "set @@tidb_partition_prune_mode = 'dynamic-only'",
        Vec::new(),
    );
    testkit.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    testkit.MustExec(
        "create table t (a int, b varchar(10), index idx_ab(a, b)) \
         partition by range (a) (\
             partition p0 values less than (10), \
             partition p1 values less than (20), \
             partition p2 values less than (30)\
         )",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values \
         (1, 'a'), (2, 'b'), (11, 'c'), (12, 'd'), (21, 'e'), (22, 'f')",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    testkit.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    domain
        .set_auto_analyze_ratio(0.1)
        .expect("set auto analyze ratio");

    let info = store.domain().stats_table("test", "t").unwrap().1;
    let global_id = info.ID;
    let p0_id = info.GetPartitionInfo().unwrap().Definitions[0].ID;
    let global = physical_stats(&store, global_id);
    let p0 = physical_stats(&store, p0_id);
    assert_eq!((global.realtime_count, global.modify_count), (6, 0));
    assert_eq!((p0.realtime_count, p0.modify_count), (2, 0));

    testkit.MustExec("insert into t values (3, 'g')", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    let global = physical_stats(&store, global_id);
    let p0 = physical_stats(&store, p0_id);
    assert_eq!((global.realtime_count, global.modify_count), (7, 1));
    assert_eq!((p0.realtime_count, p0.modify_count), (3, 1));

    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    let global = physical_stats(&store, global_id);
    let p0 = physical_stats(&store, p0_id);
    assert_eq!((global.realtime_count, global.modify_count), (7, 0));
    assert_eq!((p0.realtime_count, p0.modify_count), (3, 0));
}

/// Go `TestAutoAnalyzeRatio`, using physical stats because
/// `SHOW STATS_HEALTHY` is not implemented by the mock SQL runtime.
/// 用物理 modify/realtime 比例替代 SHOW STATS_HEALTHY，验证阈值上下触发自动分析。
#[test]
fn go_test_auto_analyze_ratio() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    let _min = AutoAnalyzeMinCntGuard::set(0);
    domain
        .set_auto_analyze_ratio(0.5)
        .expect("set auto analyze ratio");
    // PK enables row-targeted deletes in place of Go `DELETE ... LIMIT`.
    testkit.MustExec(
        "create table t (a int primary key, index idx(a))",
        Vec::new(),
    );
    let id = table_id(&store, "t");
    for i in 0..20 {
        testkit.MustExec(&format!("insert into t values ({i})"), Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());

    // 11 / analyze_count(20) = 0.55 > 0.5 => HandleAutoAnalyze (Go NeedAnalyzeTable).
    for i in 20..31 {
        testkit.MustExec(&format!("insert into t values ({i})"), Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    let stats = physical_stats(&store, id);
    assert_eq!((stats.realtime_count, stats.modify_count), (31, 11));
    let health = ((1.0 - stats.modify_count as f64 / stats.analyze_count as f64) * 100.0) as i64;
    assert_eq!(health, 44);
    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    assert_eq!(physical_stats(&store, id).modify_count, 0);

    // 12 / analyze_count(31) ≈ 0.39 < 0.5 => skip
    for i in 0..12 {
        testkit.MustExec(&format!("delete from t where a = {i}"), Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    let stats = physical_stats(&store, id);
    let health = ((1.0 - stats.modify_count as f64 / stats.analyze_count as f64) * 100.0) as i64;
    assert_eq!(health, 61);
    assert!(!domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));

    // 16 / 31 ≈ 0.52 > 0.5 => analyze
    for i in 12..16 {
        testkit.MustExec(&format!("delete from t where a = {i}"), Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    let stats = physical_stats(&store, id);
    let health = ((1.0 - stats.modify_count as f64 / stats.analyze_count as f64) * 100.0) as i64;
    assert_eq!(health, 48);
    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    assert_eq!(physical_stats(&store, id).modify_count, 0);
}

/// Go `TestDumpColumnStatsUsage`.
/// 谓词列使用量刷入 KV 后，SHOW column_stats_usage 能看到对应列与最近使用时间。
#[test]
fn go_test_dump_column_stats_usage() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec("create table t1(a int, b int)", Vec::new());
    testkit.MustExec("create table t2(a int, b int)", Vec::new());
    testkit.MustExec("create table t3(a int, b int) partition by range(a) (partition p0 values less than (10), partition p1 values less than maxvalue)", Vec::new());
    testkit.MustExec("insert into t1 values (1, 2), (3, 4)", Vec::new());
    testkit.MustExec("insert into t2 values (5, 6), (7, 8)", Vec::new());
    testkit.MustExec(
        "insert into t3 values (1, 2), (3, 4), (11, 12), (13, 14)",
        Vec::new(),
    );
    testkit.MustExec("select * from t1 where a > 1", Vec::new());
    testkit.MustExec("select * from t2 where b < 10", Vec::new());

    domain
        .dump_col_stats_usage_to_kv()
        .expect("dump predicate-column usage");
    let t1 = testkit
        .MustQuery(
            "show column_stats_usage where db_name = 'test' and table_name = 't1'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(t1.len(), 1);
    assert_eq!(t1[0][3], "a");
    assert_ne!(t1[0][4], "<nil>");
    assert_eq!(t1[0][5], "<nil>");
    let t2 = testkit
        .MustQuery(
            "show column_stats_usage where db_name = 'test' and table_name = 't2'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(t2.len(), 1);
    assert_eq!(t2[0][3], "b");
    assert_ne!(t2[0][4], "<nil>");
    assert_eq!(t2[0][5], "<nil>");

    testkit.MustExec("select * from t1 where b > 1", Vec::new());
    domain
        .dump_col_stats_usage_to_kv()
        .expect("dump additional predicate-column usage");
    let t1 = testkit
        .MustQuery(
            "show column_stats_usage where db_name = 'test' and table_name = 't1'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(t1.len(), 2);
    assert_eq!(t1[0][3], "a");
    assert_ne!(t1[0][4], "<nil>");
    assert_eq!(t1[1][3], "b");
    assert_ne!(t1[1][4], "<nil>");
    testkit.MustExec("analyze table t1", Vec::new());
    let t1 = testkit
        .MustQuery(
            "show column_stats_usage where db_name = 'test' and table_name = 't1'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(t1.len(), 2);
    assert_eq!(t1[0][..4], ["test", "t1", "", "a"]);
    assert_ne!(t1[0][4], "<nil>");
    assert_ne!(t1[0][5], "<nil>");
    assert_eq!(t1[1][..4], ["test", "t1", "", "b"]);
    assert_ne!(t1[1][4], "<nil>");
    assert_ne!(t1[1][5], "<nil>");

    for mode in ["static", "dynamic"] {
        testkit.MustExec(
            &format!("set @@tidb_partition_prune_mode = '{mode}'"),
            Vec::new(),
        );
        testkit.MustExec("delete from mysql.column_stats_usage", Vec::new());
        testkit.MustExec("select * from t3 where a < 5", Vec::new());
        domain
            .dump_col_stats_usage_to_kv()
            .expect("dump partition predicate-column usage");
        let rows = testkit
            .MustQuery(
                "show column_stats_usage where db_name = 'test' and table_name = 't3'",
                Vec::new(),
            )
            .Rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][2], "global");
        assert_eq!(rows[0][3], "a");
        assert_ne!(rows[0][4], "<nil>");
        assert_eq!(rows[0][5], "<nil>");
    }

    testkit.MustExec("delete from mysql.column_stats_usage", Vec::new());
    testkit.MustExec(
        "select * from t2 where t2.a > (select count(*) from t1 where t1.b > 1)",
        Vec::new(),
    );
    domain
        .dump_col_stats_usage_to_kv()
        .expect("dump non-correlated subquery predicate-column usage");
    let t1 = testkit
        .MustQuery(
            "show column_stats_usage where db_name = 'test' and table_name = 't1'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(t1.len(), 1);
    assert_eq!(t1[0][..4], ["test", "t1", "", "b"]);
    assert_ne!(t1[0][4], "<nil>");
    assert_eq!(t1[0][5], "<nil>");
    let t2 = testkit
        .MustQuery(
            "show column_stats_usage where db_name = 'test' and table_name = 't2'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(t2.len(), 1);
    assert_eq!(t2[0][..4], ["test", "t2", "", "a"]);
    assert_ne!(t2[0][4], "<nil>");
    assert_eq!(t2[0][5], "<nil>");
}

/// Go `TestCollectPredicateColumnsFromExecute`.
/// PREPARE 不收集谓词列；EXECUTE 收集；计划缓存命中时二次 EXECUTE 不再收集。
#[test]
fn go_test_collect_predicate_columns_from_execute() {
    for plan_cache in [false, true] {
        let (mut testkit, store, _guard) = new_mock_store_and_domain();
        let domain = store.domain();
        testkit.MustExec(
            &format!(
                "set tidb_enable_prepared_plan_cache={}",
                if plan_cache { "ON" } else { "OFF" }
            ),
            Vec::new(),
        );

        testkit.MustExec("create table t1(a int, b int)", Vec::new());
        testkit.MustExec(
            "prepare stmt from 'select * from t1 where a > ?'",
            Vec::new(),
        );
        domain
            .dump_col_stats_usage_to_kv()
            .expect("dump predicate-column usage after PREPARE");
        // Prepare only converts sql string to ast and doesn't do optimization,
        // so no predicate column is collected.
        assert!(
            testkit
                .MustQuery(
                    "show column_stats_usage where db_name = 'test' and table_name = 't1'",
                    Vec::new(),
                )
                .Rows()
                .is_empty()
        );
        testkit.MustExec("set @p1 = 1", Vec::new());
        testkit.MustExec("execute stmt using @p1", Vec::new());
        domain
            .dump_col_stats_usage_to_kv()
            .expect("dump predicate-column usage after EXECUTE");
        let rows = testkit
            .MustQuery(
                "show column_stats_usage where db_name = 'test' and table_name = 't1'",
                Vec::new(),
            )
            .Rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][..4], ["test", "t1", "", "a"]);
        assert_ne!(rows[0][4], "<nil>");
        assert_eq!(rows[0][5], "<nil>");

        testkit.MustExec("delete from mysql.column_stats_usage", Vec::new());
        testkit.MustExec("set @p2 = 2", Vec::new());
        testkit.MustExec("execute stmt using @p2", Vec::new());
        if plan_cache {
            testkit
                .MustQuery("select @@last_plan_from_cache", Vec::new())
                .Check(vec![vec!["1".to_owned()]]);
            domain
                .dump_col_stats_usage_to_kv()
                .expect("dump predicate-column usage after cached EXECUTE");
            // If the second execution uses the cached plan, no predicate column
            // is collected.
            assert!(
                testkit
                    .MustQuery(
                        "show column_stats_usage where db_name = 'test' and table_name = 't1'",
                        Vec::new(),
                    )
                    .Rows()
                    .is_empty()
            );
        } else {
            testkit
                .MustQuery("select @@last_plan_from_cache", Vec::new())
                .Check(vec![vec!["0".to_owned()]]);
            domain
                .dump_col_stats_usage_to_kv()
                .expect("dump predicate-column usage after re-planned EXECUTE");
            let rows = testkit
                .MustQuery(
                    "show column_stats_usage where db_name = 'test' and table_name = 't1'",
                    Vec::new(),
                )
                .Rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][..4], ["test", "t1", "", "a"]);
            assert_ne!(rows[0][4], "<nil>");
            assert_eq!(rows[0][5], "<nil>");
        }
    }
}

/// Go `TestColumnTracking`.
/// 多次查询不同谓词列后 dump，usage 表累积展示所有被引用列。
#[test]
fn go_test_column_tracking() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec("create table t(a int, b int, c int)", Vec::new());
    testkit.MustExec("select * from t where b > 1", Vec::new());
    domain
        .dump_col_stats_usage_to_kv()
        .expect("dump first predicate-column usage");
    let rows = testkit
        .MustQuery(
            "show column_stats_usage where db_name = 'test' and table_name = 't'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][3], "b");

    testkit.MustExec("select * from t where b < 1 and c > 1", Vec::new());
    domain
        .dump_col_stats_usage_to_kv()
        .expect("dump additional predicate-column usage");
    let mut columns = testkit
        .MustQuery(
            "show column_stats_usage where db_name = 'test' and table_name = 't'",
            Vec::new(),
        )
        .Rows()
        .into_iter()
        .map(|row| row[3].clone())
        .collect::<Vec<_>>();
    columns.sort();
    assert_eq!(columns, vec!["b", "c"]);
}

/// Go `TestAutoAnalyzePartitionTableAfterAddingIndex`.
/// 分区表加索引后缺少索引统计，自动分析应补齐全局索引直方图条目。
#[test]
fn go_test_auto_analyze_partition_table_after_adding_index() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    let _min = AutoAnalyzeMinCntGuard::set(0);
    testkit.MustExec("set global tidb_analyze_version = 2", Vec::new());
    testkit.MustExec(
        "set global tidb_partition_prune_mode = 'dynamic'",
        Vec::new(),
    );
    testkit.MustExec(
        "create table t (a int, b int) partition by range (a) \
         (PARTITION p0 VALUES LESS THAN (10), PARTITION p1 VALUES LESS THAN MAXVALUE)",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values (1,2), (3,4), (11,12),(13,14)",
        Vec::new(),
    );
    testkit.MustExec("set session tidb_analyze_version = 2", Vec::new());
    testkit.MustExec(
        "set session tidb_partition_prune_mode = 'dynamic'",
        Vec::new(),
    );
    for column in ["a", "b"] {
        testkit.MustExec(&format!("select * from t where {column} = '1'"), Vec::new());
    }
    domain
        .dump_col_stats_usage_to_kv()
        .expect("TriggerPredicateColumnsCollection");
    testkit.MustExec("analyze table t", Vec::new());
    assert!(!domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));

    testkit.MustExec("alter table t add index idx(a)", Vec::new());
    let info = store.domain().stats_table("test", "t").unwrap().1;
    let index_id = info.Indices[0].ID;
    assert!(
        !physical_stats(&store, info.ID)
            .indexes
            .contains_key(&index_id)
    );
    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    assert!(
        physical_stats(&store, info.ID)
            .indexes
            .contains_key(&index_id)
    );
}

/// Go `TestOutOfOrderUpdate`.
/// 手工改写 stats_meta.count 模拟乱序；后续 delta flush 不应把 count 算错或回退。
#[test]
fn go_test_out_of_order_update() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec("create table t (a int, b int)", Vec::new());
    testkit.MustExec("insert into t values (1,2)", Vec::new());
    let key_id = table_id(&store, "t");

    testkit.MustExec("insert into t values (2,2),(4,5)", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    domain
        .restricted_stats_execute(
            &format!("update mysql.stats_meta set count = 1 where table_id = {key_id}"),
            &[],
        )
        .expect("update stats_meta count");

    testkit.MustExec("delete from t", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(
        testkit
            .MustQuery(
                &format!(
                    "select count from mysql.stats_meta where table_id = {}",
                    key_id
                ),
                Vec::new(),
            )
            .Rows(),
        vec![vec!["0".to_owned()]]
    );

    domain
        .restricted_stats_execute(
            &format!("update mysql.stats_meta set count = 3 where table_id = {key_id}"),
            &[],
        )
        .expect("update stats_meta count");
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(
        testkit
            .MustQuery(
                &format!(
                    "select count from mysql.stats_meta where table_id = {}",
                    key_id
                ),
                Vec::new(),
            )
            .Rows(),
        vec![vec!["3".to_owned()]]
    );
}

/// Go `TestStatsLockForDelta`.
/// 锁定统计后插入不更新 realtime_count；解锁并 analyze 后恢复正常增量。
#[test]
fn go_test_stats_lock_for_delta() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    testkit.MustExec("set @@session.tidb_analyze_version = 2", Vec::new());
    testkit.MustExec("create table t1 (c1 int, c2 int)", Vec::new());
    testkit.MustExec("create table t2 (c1 int, c2 int)", Vec::new());
    let key1_id = table_id(&store, "t1");
    let key2_id = table_id(&store, "t2");

    testkit.MustExec("lock stats t1", Vec::new());
    let row_count1 = 10;
    let row_count2 = 20;
    for _ in 0..row_count1 {
        testkit.MustExec("insert into t1 values(1, 2)", Vec::new());
    }
    for _ in 0..row_count2 {
        testkit.MustExec("insert into t2 values(1, 2)", Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(physical_stats(&store, key1_id).realtime_count, 0);
    assert_eq!(physical_stats(&store, key2_id).realtime_count, row_count2);

    testkit.MustExec("analyze table t1", Vec::new());
    for _ in 0..row_count1 {
        testkit.MustExec("insert into t1 values(1, 2)", Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(physical_stats(&store, key1_id).realtime_count, 0);

    testkit.MustExec("unlock stats t1", Vec::new());
    testkit.MustExec("analyze table t1", Vec::new());
    assert_eq!(physical_stats(&store, key1_id).realtime_count, 20);

    for _ in 0..row_count1 {
        testkit.MustExec("insert into t1 values(1, 2)", Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert_eq!(physical_stats(&store, key1_id).realtime_count, 30);
}

/// Go `TestFillMissingStatsMeta`.
/// 普通表与分区表 flush / 选择性 dump 时正确填充或更新 stats_meta 版本与计数。
#[test]
fn go_test_fill_missing_stats_meta() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec("create table t1 (a int, b int)", Vec::new());
    testkit.MustExec(
        "create table t2 (a int, b int) partition by range (a) \
         (partition p0 values less than (10), partition p1 values less than (maxvalue))",
        Vec::new(),
    );
    assert!(
        testkit
            .MustQuery("select * from mysql.stats_meta", Vec::new())
            .Rows()
            .is_empty()
    );

    let key1_id = table_id(&store, "t1");
    let (key2_id, info) = {
        let (k, i) = store.domain().stats_table("test", "t2").unwrap();
        (k.table_id, i)
    };
    let partitions = info.GetPartitionInfo().unwrap();
    assert_eq!(partitions.Definitions.len(), 2);
    let p0 = partitions.Definitions[0].ID;
    let p1 = partitions.Definitions[1].ID;

    let check_stats_meta = |testkit: &mut TestKit, id: i64, modify: &str, count: &str| -> i64 {
        let rows = testkit
            .MustQuery(
                &format!(
                    "select version, modify_count, count from mysql.stats_meta where table_id = {id}"
                ),
                Vec::new(),
            )
            .Rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], modify);
        assert_eq!(rows[0][2], count);
        rows[0][0].parse::<i64>().unwrap()
    };

    testkit.MustExec("insert into t1 values (1, 2), (3, 4)", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    let ver1 = check_stats_meta(&mut testkit, key1_id, "2", "2");
    testkit.MustExec("delete from t1 where a = 1", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    let ver2 = check_stats_meta(&mut testkit, key1_id, "3", "1");
    assert!(ver2 > ver1);

    testkit.MustExec("insert into t2 values (1, 2), (3, 4)", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    check_stats_meta(&mut testkit, p0, "2", "2");
    let global_ver1 = check_stats_meta(&mut testkit, key2_id, "2", "2");
    testkit.MustExec("insert into t2 values (11, 12)", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    check_stats_meta(&mut testkit, p1, "1", "1");
    let global_ver2 = check_stats_meta(&mut testkit, key2_id, "3", "3");
    assert!(global_ver2 > global_ver1);

    testkit.MustExec("insert into t1 values (5, 6)", Vec::new());
    testkit.MustExec("insert into t2 values (5, 6), (15, 16)", Vec::new());
    // Targeted dump of specific physical IDs.
    domain
        .flush_stats_delta_history(&[key1_id, p1])
        .expect("DumpStatsDeltaToKV selective");
    check_stats_meta(&mut testkit, key1_id, "4", "2");
    check_stats_meta(&mut testkit, p0, "2", "2");
    check_stats_meta(&mut testkit, p1, "2", "2");
    let global_ver3 = check_stats_meta(&mut testkit, key2_id, "4", "4");
    assert!(global_ver3 > global_ver2);

    domain
        .dump_stats_delta_to_kv(true)
        .expect("DumpStatsDeltaToKV");
    check_stats_meta(&mut testkit, p0, "3", "3");
    let global_ver4 = check_stats_meta(&mut testkit, key2_id, "5", "5");
    assert!(global_ver4 > global_ver3);
}

/// Go `TestNotDumpSysTable`.
/// 删除 stats_meta 行后强制 flush，系统表自身增量不得重建自己的 meta 行。
#[test]
fn go_test_not_dump_sys_table() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec("create table t1 (a int, b int)", Vec::new());
    // Go's test explicitly handles the pending DDL event before asserting
    // that the user table has a stats_meta row.
    let table_id = table_id(&store, "t1");
    domain
        .persist_stats_meta(&[table_id])
        .expect("handle CREATE TABLE statistics DDL event");
    let rows = testkit
        .MustQuery("select table_id from mysql.stats_meta", Vec::new())
        .Rows();
    assert_eq!(rows.len(), 1);
    // Go deletes directly against mysql.stats_meta; session routes this through
    // restricted statistics SQL. That also removes handle meta and leaves orphan
    // pending deltas which DumpStatsDeltaToKV must skip (Go needDumpStatsDelta).
    testkit.MustExec("delete from mysql.stats_meta", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    // Sys-table deltas must not recreate mysql.stats_meta for itself.
    let sys_id = domain
        .stats_table("mysql", "stats_meta")
        .map(|(key, _)| key.table_id)
        .expect("mysql.stats_meta is present in the information schema");
    assert!(
        testkit
            .MustQuery(
                &format!("select table_id from mysql.stats_meta where table_id = {sys_id}"),
                Vec::new(),
            )
            .Rows()
            .is_empty()
    );
}

/// Go `TestStatsLockUnlockForAutoAnalyze` (lock skips auto-analyze).
/// 统计锁期间跳过自动分析且 modify_count 不变；解锁后可手动 analyze 收敛行数。
#[test]
fn go_test_stats_lock_unlock_for_auto_analyze() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    let _min = AutoAnalyzeMinCntGuard::set(0);
    domain
        .set_auto_analyze_ratio(0.5)
        .expect("set auto analyze ratio");
    testkit.MustExec(
        "create table t (a int primary key, index idx(a))",
        Vec::new(),
    );
    for i in 0..20 {
        testkit.MustExec(&format!("insert into t values ({i})"), Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());

    for i in 20..31 {
        testkit.MustExec(&format!("insert into t values ({i})"), Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));

    let key_id = table_id(&store, "t");
    let before = physical_stats(&store, key_id);
    assert!(
        before
            .columns
            .values()
            .all(|column| column.analyzed_or_synthesized)
    );
    testkit.MustExec("lock stats t", Vec::new());
    // Go uses `delete from t limit 12`; PK deletes stand in.
    for i in 0..12 {
        testkit.MustExec(&format!("delete from t where a = {i}"), Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    assert!(!domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    let after_lock = physical_stats(&store, key_id);
    assert_eq!(before.modify_count, after_lock.modify_count);

    testkit.MustExec("unlock stats t", Vec::new());
    for i in 12..16 {
        testkit.MustExec(&format!("delete from t where a = {i}"), Vec::new());
    }
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    // Go asserts against `select count(*)` (15). Mock session cannot SELECT user
    // tables; 31 - 12 - 4 = 15 matches Go after the same deletes.
    assert_eq!(physical_stats(&store, key_id).realtime_count, 15);
}

/// 构造确定性 TopN 列表及期望聚合计数，供 MergeTopN 用例复现。
fn deterministic_top_ns(
    top_n_count: usize,
    entries_per_top_n: usize,
    value_limit: usize,
) -> (Vec<astersql_statistics::TopN>, HashMap<usize, u64>) {
    assert!(value_limit >= entries_per_top_n);
    let mut expected = HashMap::new();
    let mut top_ns = Vec::with_capacity(top_n_count);
    for top_n_index in 0..top_n_count {
        let mut top_n = NewTopN(entries_per_top_n);
        for entry_index in 0..entries_per_top_n {
            let value = (top_n_index * 3 + entry_index * 7) % value_limit;
            assert!(
                !top_n
                    .TopN
                    .iter()
                    .any(|entry| entry.Encoded == value.to_string().as_bytes())
            );
            let count = 1 + ((top_n_index * 19 + entry_index * 23) % 99) as u64;
            *expected.entry(value).or_default() += count;
            top_n.TopN.push(TopNMeta {
                Encoded: value.to_string().into_bytes(),
                Count: count,
            });
        }
        top_ns.push(top_n);
    }
    (top_ns, expected)
}

/// Go `TestMergeTopN`.
/// 合并多份 TopN：保留条数上限、溢出桶计数正确，且溢出项计数不超过合并最小值。
#[test]
fn go_test_merge_top_n() {
    for (top_n_count, entries_per_top_n, value_limit) in
        [(10, 5, 50), (1, 5, 50), (5, 5, 5), (5, 5, 10)]
    {
        let (top_ns, expected) = deterministic_top_ns(top_n_count, entries_per_top_n, value_limit);
        let refs = top_ns.iter().collect::<Vec<_>>();
        let (merged, spilled) = MergeTopN(&refs, entries_per_top_n as u32);
        let merged = merged.expect("non-empty inputs produce a merged TopN");
        assert_eq!(merged.TopN.len(), entries_per_top_n.min(expected.len()));
        assert_eq!(merged.TopN.len() + spilled.len(), expected.len());
        for entry in merged.TopN.iter().chain(&spilled) {
            let value = std::str::from_utf8(&entry.Encoded)
                .unwrap()
                .parse::<usize>()
                .unwrap();
            assert_eq!(entry.Count, expected[&value]);
        }
        let minimum_merged = merged.TopN.iter().map(|entry| entry.Count).min().unwrap();
        assert!(spilled.iter().all(|entry| entry.Count <= minimum_merged));
    }
}

/// 向直方图追加一个整数下/上界桶（计数为 0），用于 SplitRange 用例。
fn append_bucket(histogram: &mut astersql_statistics::Histogram, lower: i64, upper: i64) {
    histogram.AppendBucket(&NewIntDatum(lower), &NewIntDatum(upper), 0, 0);
}

/// 将 HistogramRange 格式化为 `(low,high]` 风格字符串便于断言。
fn range_string(range: &HistogramRange) -> String {
    format!(
        "{}{},{}{}",
        if range.LowExclude { '(' } else { '[' },
        range.LowVal[0].GetInt64(),
        range.HighVal[0].GetInt64(),
        if range.HighExclude { ')' } else { ']' }
    )
}

/// Go `TestSplitRange`.
/// 按直方图桶边界切分查询区间；类型不匹配时 matched=false 且区间原样返回。
#[test]
fn go_test_split_range() {
    let mut histogram = NewHistogram(0, 0, 0, 0, &FieldType::default(), 5, 0);
    for (low, high) in [(1, 1), (2, 5), (7, 7), (8, 8), (10, 13)] {
        append_bucket(&mut histogram, low, high);
    }

    let cases = [
        (vec![1, 1], vec![false, false], "[1,1]"),
        (
            vec![0, 1, 3, 8, 8, 20],
            vec![true, false, true, false, true, false],
            "(0,1],(3,7),[7,8),[8,8],(8,10),[10,20]",
        ),
        (
            vec![8, 10, 20, 30],
            vec![false, false, true, true],
            "[8,10),[10,10],(20,30)",
        ),
        (vec![8, 9], vec![false, true], "[8,9)"),
    ];
    for (points, excludes, expected) in cases {
        let ranges = points
            .chunks_exact(2)
            .enumerate()
            .map(|(index, points)| HistogramRange {
                LowVal: vec![NewIntDatum(points[0])],
                HighVal: vec![NewIntDatum(points[1])],
                LowExclude: excludes[index * 2],
                HighExclude: excludes[index * 2 + 1],
            })
            .collect::<Vec<_>>();
        let (ranges, matched) = histogram.SplitRange(&ranges);
        assert!(matched);
        assert_eq!(
            ranges
                .iter()
                .map(range_string)
                .collect::<Vec<_>>()
                .join(","),
            expected
        );
    }

    let mismatched = HistogramRange {
        LowVal: vec![NewStringDatum("a".to_owned())],
        HighVal: vec![NewStringDatum("z".to_owned())],
        LowExclude: false,
        HighExclude: false,
    };
    let (unchanged, matched) = histogram.SplitRange(std::slice::from_ref(&mismatched));
    assert!(!matched);
    assert_eq!(unchanged.len(), 1);
}

/// 将两个 f64 计数相减并转为 i64，对齐 Go 辅助函数。
fn subtraction(new_counter: f64, old_counter: f64) -> i64 {
    (new_counter - old_counter) as i64
}

/// Go helper `subtraction`.
/// 校验正负与零差三种减法结果。
#[test]
fn go_helper_subtraction() {
    assert_eq!(subtraction(17.0, 5.0), 12);
    assert_eq!(subtraction(5.0, 17.0), -12);
    assert_eq!(subtraction(9.0, 9.0), 0);
}
