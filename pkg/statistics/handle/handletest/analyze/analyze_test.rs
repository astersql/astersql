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

// ANALYZE 与分区全局统计的集成测试。
//
// 验证带 TopN/buckets 选项的全局统计形状、动态裁剪下的分区/索引 ANALYZE、
// FMSketch 落库，以及手动/自动 ANALYZE 的作业计数指标。

use std::sync::{Arc, LazyLock, Mutex};

use astersql_domain::Domain;
use astersql_domain::metrics::compat_prometheus::CounterVec;
use astersql_domain::metrics::stats;
use astersql_statistics::{ResetAutoAnalyzeMinCnt, SetAutoAnalyzeMinCnt};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};

use crate::main_test::setup_common_tests;

/// 这些集成测试会修改全局系统变量与共享 Prometheus 指标，须与 Go 的串行测试语义一致。
static ANALYZE_TEST_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// 串行化修改全局状态的测试；前一用例失败时仍允许其余用例独立报告结果。
fn lock_analyze_tests() -> std::sync::MutexGuard<'static, ()> {
    ANALYZE_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 初始化公共测试环境，并创建 mock store、Domain 与 TestKit。
fn new_case() -> (Arc<AnalyzeStatsStore>, Arc<Domain>, TestKit) {
    setup_common_tests();
    let (store, domain) = CreateMockStoreAndDomain();
    let testkit = TestKit::new(store.clone());
    (store, domain, testkit)
}

/// 检查指定物理分区（或 `global`）上列/索引的 TopN 个数与桶数是否落在期望容差内。
fn check_for_global_stats_with_opts(
    domain: &Domain,
    db: &str,
    table: &str,
    partition: &str,
    topn: usize,
    buckets: usize,
) {
    let info = domain
        .table_by_name(db, table)
        .unwrap_or_else(|error| panic!("lookup {db}.{table}: {error}"));
    let mut physical_id = info.ID;
    if partition != "global" {
        let found = info
            .GetPartitionInfo()
            .into_iter()
            .flat_map(|part| part.Definitions.iter())
            .find(|definition| definition.Name.L == partition)
            .unwrap_or_else(|| panic!("partition {partition} missing"));
        physical_id = found.ID;
    }
    let stats = domain
        .stats_context()
        .persisted_physical_stats(physical_id)
        .unwrap_or_else(|| panic!("missing stats for {db}.{table}[{partition}]"));
    // 桶数允许一定浮动：ANALYZE 采样与合并可能导致实际桶数略偏期望值。
    let delta = buckets / 2 + 10;
    for index in stats.indexes.values() {
        if index.buckets.is_empty() {
            continue;
        }
        let num_topn = index.top_n.len();
        let num_buckets = index.buckets.len();
        assert_eq!(topn, num_topn, "index topn for {partition}");
        assert!(
            num_buckets >= buckets.saturating_sub(delta),
            "index buckets {num_buckets} < {} for {partition}",
            buckets.saturating_sub(delta)
        );
        assert!(
            num_buckets <= buckets + delta,
            "index buckets {num_buckets} > {} for {partition}",
            buckets + delta
        );
    }
    for column in stats.columns.values() {
        if column.buckets.is_empty() {
            continue;
        }
        let num_topn = column.top_n.len();
        let num_buckets = column.buckets.len();
        assert_eq!(topn, num_topn, "column topn for {partition}");
        assert!(
            num_buckets >= buckets.saturating_sub(delta),
            "column buckets {num_buckets} < {} for {partition}",
            buckets.saturating_sub(delta)
        );
        assert!(
            num_buckets <= buckets + delta,
            "column buckets {num_buckets} > {} for {partition}",
            buckets + delta
        );
    }
}

/// 构造两分区 range 表并写入倾斜数据，开启 analyze v2 与动态裁剪。
fn prepare_for_global_stats_with_opts(testkit: &mut TestKit, tbl_name: &str, db_name: &str) {
    testkit.MustExec(
        &format!("create database if not exists {db_name}"),
        Vec::new(),
    );
    testkit.MustExec(&format!("use {db_name}"), Vec::new());
    testkit.MustExec(&format!("drop table if exists {tbl_name}"), Vec::new());
    testkit.MustExec(
        &format!(
            "create table {tbl_name} (a int, key(a)) partition by range (a) \
             (partition p0 values less than (100000), partition p1 values less than (200000))"
        ),
        Vec::new(),
    );
    let mut buf1 = format!("insert into {tbl_name} values (0)");
    let mut buf2 = format!("insert into {tbl_name} values (100000)");
    for i in (0..5000).step_by(3) {
        buf1.push_str(&format!(", ({i})"));
        buf2.push_str(&format!(", ({})", 100000 + i));
    }
    for _ in 0..1000 {
        buf1.push_str(", (0)");
        buf2.push_str(", (100000)");
    }
    testkit.MustExec(&buf1, Vec::new());
    testkit.MustExec(&buf2, Vec::new());
    testkit.MustExec("set @@tidb_analyze_version=2", Vec::new());
    testkit.MustExec("set @@tidb_partition_prune_mode='dynamic'", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
}

/// 类似 `prepare_for_global_stats_with_opts`，但额外写入重复高频值以便稳定 TopN。
fn prepare_for_global_stats_with_opts_v2(testkit: &mut TestKit, tbl_name: &str, db_name: &str) {
    testkit.MustExec(
        &format!("create database if not exists {db_name}"),
        Vec::new(),
    );
    testkit.MustExec(&format!("use {db_name}"), Vec::new());
    testkit.MustExec(&format!("drop table if exists {tbl_name}"), Vec::new());
    testkit.MustExec(
        &format!(
            "create table {tbl_name} (a int, key(a)) partition by range (a) \
             (partition p0 values less than (100000), partition p1 values less than (200000))"
        ),
        Vec::new(),
    );
    let mut buf1 = format!("insert into {tbl_name} values (0)");
    let mut buf2 = format!("insert into {tbl_name} values (100000)");
    for _ in 0..1000 {
        buf1.push_str(", (2), (1), (0)");
        buf2.push_str(", (100002), (100001), (100000)");
    }
    for i in (0..5000).step_by(3) {
        buf1.push_str(&format!(", ({i})"));
        buf2.push_str(&format!(", ({})", 100000 + i));
    }
    testkit.MustExec(&buf1, Vec::new());
    testkit.MustExec(&buf2, Vec::new());
    testkit.MustExec("set @@tidb_analyze_version=2", Vec::new());
    testkit.MustExec("set @@tidb_partition_prune_mode='dynamic'", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
}

/// 临时关闭 `tidb_persist_analyze_options`，并在测试退出（包括 panic）时恢复全局值。
struct PersistAnalyzeOptionsGuard<'a> {
    testkit: &'a mut TestKit,
    original: String,
}

impl<'a> PersistAnalyzeOptionsGuard<'a> {
    fn disable(testkit: &'a mut TestKit) -> Self {
        let original = testkit
            .MustQuery("select @@tidb_persist_analyze_options", Vec::new())
            .Rows()[0][0]
            .clone();
        testkit.MustExec("set global tidb_persist_analyze_options=false", Vec::new());
        Self { testkit, original }
    }

    fn testkit(&mut self) -> &mut TestKit {
        self.testkit
    }
}

impl Drop for PersistAnalyzeOptionsGuard<'_> {
    fn drop(&mut self) {
        self.testkit.MustExec(
            &format!("set global tidb_persist_analyze_options={}", self.original),
            Vec::new(),
        );
    }
}

#[test]
/// 覆盖生成列上的 ANALYZE：先将谓词列使用落盘，再断言直方图形状。
fn TestAnalyzeVirtualCol() {
    let _guard = lock_analyze_tests();
    let (_store, domain, mut tk) = new_case();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec(
        "create table t(a int, b int generated always as (-a) virtual, \
         c int generated always as (-a) stored, index (c))",
        Vec::new(),
    );
    tk.MustExec("insert into t(a) values(2),(1),(1),(3),(NULL)", Vec::new());
    for column in ["a", "b", "c"] {
        tk.MustExec(&format!("select * from t where {column} = '1'"), Vec::new());
    }
    domain
        .dump_col_stats_usage_to_kv()
        .expect("TriggerPredicateColumnsCollection");
    tk.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    let rows = tk
        .MustQuery("show stats_histograms where table_name ='t'", Vec::new())
        .Rows();
    assert_eq!(
        rows.len(),
        3,
        "expected generated-column histograms: {rows:?}"
    );
}

#[test]
/// 遍历合法/非法 TopN、buckets 组合，校验全局与分区统计形状或报错。
fn TestAnalyzeGlobalStatsWithOpts1() {
    let _guard = lock_analyze_tests();
    let (_store, domain, mut tk) = new_case();
    prepare_for_global_stats_with_opts(&mut tk, "test_gstats_opt", "test_gstats_opt");
    tk.MustQuery("select database()", Vec::new())
        .Check(vec![vec!["test_gstats_opt"]]);

    struct Opt {
        topn: i64,
        buckets: i64,
        err: bool,
    }
    let cases = [
        Opt {
            topn: 1,
            buckets: 37,
            err: false,
        },
        Opt {
            topn: 2,
            buckets: 47,
            err: false,
        },
        Opt {
            topn: 10,
            buckets: 77,
            err: false,
        },
        Opt {
            topn: 77,
            buckets: 219,
            err: false,
        },
        Opt {
            topn: -31,
            buckets: 222,
            err: true,
        },
        Opt {
            topn: 10,
            buckets: -77,
            err: true,
        },
        Opt {
            topn: 100001,
            buckets: 47,
            err: true,
        },
        Opt {
            topn: 77,
            buckets: 100001,
            err: true,
        },
    ];
    for ca in cases {
        let sql = format!(
            "analyze table test_gstats_opt with {} topn, {} buckets",
            ca.topn, ca.buckets
        );
        if !ca.err {
            tk.MustExec(&sql, Vec::new());
            check_for_global_stats_with_opts(
                &domain,
                "test_gstats_opt",
                "test_gstats_opt",
                "global",
                ca.topn as usize,
                ca.buckets as usize,
            );
            check_for_global_stats_with_opts(
                &domain,
                "test_gstats_opt",
                "test_gstats_opt",
                "p0",
                ca.topn as usize,
                ca.buckets as usize,
            );
            check_for_global_stats_with_opts(
                &domain,
                "test_gstats_opt",
                "test_gstats_opt",
                "p1",
                ca.topn as usize,
                ca.buckets as usize,
            );
        } else {
            assert!(!tk.ExecToErr(&sql).message().is_empty());
        }
    }
}

#[test]
/// 验证整表与单分区 ANALYZE 选项变更后，全局统计取各分区选项的合并效果。
fn TestAnalyzeGlobalStatsWithOpts2() {
    let _guard = lock_analyze_tests();
    let (_store, domain, mut tk) = new_case();
    let mut persist_options = PersistAnalyzeOptionsGuard::disable(&mut tk);
    let tk = persist_options.testkit();
    prepare_for_global_stats_with_opts_v2(tk, "test_gstats_opt2", "test_gstats_opt2");

    tk.MustExec(
        "analyze table test_gstats_opt2 with 2 topn, 10 buckets, 1000 samples",
        Vec::new(),
    );
    check_for_global_stats_with_opts(
        &domain,
        "test_gstats_opt2",
        "test_gstats_opt2",
        "global",
        2,
        10,
    );
    check_for_global_stats_with_opts(&domain, "test_gstats_opt2", "test_gstats_opt2", "p0", 2, 10);
    check_for_global_stats_with_opts(&domain, "test_gstats_opt2", "test_gstats_opt2", "p1", 2, 10);

    tk.MustExec(
        "analyze table test_gstats_opt2 partition p0 with 3 topn, 20 buckets",
        Vec::new(),
    );
    // 只分析 p0 时，全局选项跟随本次 ANALYZE；p1 仍保留先前选项。
    check_for_global_stats_with_opts(
        &domain,
        "test_gstats_opt2",
        "test_gstats_opt2",
        "global",
        3,
        20,
    );
    check_for_global_stats_with_opts(&domain, "test_gstats_opt2", "test_gstats_opt2", "p0", 3, 20);
    check_for_global_stats_with_opts(&domain, "test_gstats_opt2", "test_gstats_opt2", "p1", 2, 10);

    tk.MustExec(
        "analyze table test_gstats_opt2 partition p1 with 1 topn, 15 buckets",
        Vec::new(),
    );
    check_for_global_stats_with_opts(
        &domain,
        "test_gstats_opt2",
        "test_gstats_opt2",
        "global",
        1,
        15,
    );
    check_for_global_stats_with_opts(&domain, "test_gstats_opt2", "test_gstats_opt2", "p0", 3, 20);
    check_for_global_stats_with_opts(&domain, "test_gstats_opt2", "test_gstats_opt2", "p1", 1, 15);

    tk.MustExec(
        "analyze table test_gstats_opt2 partition p0 with 2 topn, 10 buckets",
        Vec::new(),
    );
    check_for_global_stats_with_opts(
        &domain,
        "test_gstats_opt2",
        "test_gstats_opt2",
        "global",
        2,
        10,
    );
    check_for_global_stats_with_opts(&domain, "test_gstats_opt2", "test_gstats_opt2", "p0", 2, 10);
    check_for_global_stats_with_opts(&domain, "test_gstats_opt2", "test_gstats_opt2", "p1", 1, 15);
}

#[test]
/// 动态裁剪下整表/分区/分区索引 ANALYZE 后，全局索引桶应始终存在。
fn TestAnalyzeWithDynamicPartitionPruneMode() {
    let _guard = lock_analyze_tests();
    let (_store, _domain, mut tk) = new_case();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    tk.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    tk.MustExec("set @@global.tidb_enable_auto_analyze='OFF'", Vec::new());
    tk.MustExec(
        "create table t (a int, key(a)) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (22))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1), (2), (3), (10), (11)", Vec::new());
    tk.MustExec("analyze table t with 1 topn, 2 buckets", Vec::new());
    let rows = tk
        .MustQuery(
            "show stats_buckets where partition_name = 'global' and is_index=1",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 2, "expected two global index buckets: {rows:?}");
    assert_eq!(rows[1][6], "4");
    tk.MustExec("insert into t values (1), (2), (2)", Vec::new());
    tk.MustExec(
        "analyze table t partition p0 with 1 topn, 2 buckets",
        Vec::new(),
    );
    let rows = tk
        .MustQuery(
            "show stats_buckets where partition_name = 'global' and is_index=1",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 2, "expected two global index buckets: {rows:?}");
    assert_eq!(rows[1][6], "5");
    tk.MustExec("insert into t values (3)", Vec::new());
    tk.MustExec(
        "analyze table t partition p0 index a with 1 topn, 2 buckets",
        Vec::new(),
    );
    let rows = tk
        .MustQuery(
            "show stats_buckets where partition_name = 'global' and is_index=1",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 1, "expected one global index bucket: {rows:?}");
    assert_eq!(rows[0][6], "6");
    // Rust SET currently accepts the concrete default value, but not Go's
    // `DEFAULT` token on this global variable.
    tk.MustExec("set @@global.tidb_enable_auto_analyze='ON'", Vec::new());
}

#[test]
/// 分区 ANALYZE 后应写入 `mysql.stats_fm_sketch`（FMSketch 用于 NDV 估计）。
fn TestFMSWithAnalyzePartition() {
    let _guard = lock_analyze_tests();
    let (_store, _domain, mut tk) = new_case();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    tk.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    tk.MustExec(
        "create table t (a int, key(a)) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (22))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1), (2), (3), (10), (11)", Vec::new());
    tk.MustQuery("select count(*) from mysql.stats_fm_sketch", Vec::new())
        .Check(vec![vec!["0"]]);
    tk.MustExec(
        "analyze table t partition p0 with 1 topn, 2 buckets",
        Vec::new(),
    );
    let mut warnings = tk.MustQuery("show warnings", Vec::new());
    warnings.Sort().Check(vec![
        vec![
            "Note",
            "1105",
            "Analyze use auto adjusted sample rate 1.000000 for table test.t's partition p0, reason to use this rate is \"use min(1, 110000/10000) as the sample-rate=1\"",
        ],
        vec![
            "Warning",
            "1105",
            "Ignore columns and options when analyze partition in dynamic mode",
        ],
    ]);
    tk.MustQuery("select count(*) from mysql.stats_fm_sketch", Vec::new())
        .Check(vec![vec!["2"]]);
}

#[test]
/// 手动与自动 ANALYZE 应增加 finished 作业计数且不引入额外 failed。
fn TestAnalyzeMetricsCounters() {
    let _guard = lock_analyze_tests();
    let (_store, domain, mut tk) = new_case();
    tk.MustExec("use test", Vec::new());

    unsafe { astersql_domain::metrics::metrics::InitMetrics().expect("initialize stats metrics") };
    // 静态指标初始化后不再替换，克隆其句柄读取标签值而不移动全局所有权。
    let read_counter = |counter: *const Option<CounterVec>, label: &str| unsafe {
        let counter = std::ptr::read(counter);
        let value = counter
            .as_ref()
            .expect("stats counter must be initialized")
            .with_label_values(&[label])
            .get();
        std::mem::forget(counter);
        value
    };
    let manual_succ = || read_counter(std::ptr::addr_of!(stats::ManualAnalyzeCounter), "succ");
    let manual_failed = || read_counter(std::ptr::addr_of!(stats::ManualAnalyzeCounter), "failed");
    let auto_succ = || read_counter(std::ptr::addr_of!(stats::AutoAnalyzeCounter), "succ");
    let auto_failed = || read_counter(std::ptr::addr_of!(stats::AutoAnalyzeCounter), "failed");

    tk.MustExec("create table t_metrics_manual(a int)", Vec::new());
    // Go consumes the queued CREATE TABLE event through
    // HandleNextDDLEventWithTxn here. The Rust mock DDL path applies that
    // statistics metadata change synchronously, so no cache refresh is needed.
    tk.MustExec("insert into t_metrics_manual values (1),(2)", Vec::new());
    let before_manual_succ = manual_succ();
    let before_manual_fail = manual_failed();
    tk.MustExec("analyze table t_metrics_manual", Vec::new());
    assert_eq!(manual_succ(), before_manual_succ + 1.0);
    assert_eq!(manual_failed(), before_manual_fail);

    tk.MustExec("create table t_metrics_auto(a int)", Vec::new());
    // As above, CREATE TABLE has already installed the stats_meta row.
    tk.MustExec("insert into t_metrics_auto values (1)", Vec::new());
    tk.MustExec("flush stats_delta *.*", Vec::new());
    domain
        .update_stats()
        .expect("refresh statistics before auto analyze");

    SetAutoAnalyzeMinCnt(0);
    let _guard = AutoAnalyzeMinCntGuard;
    tk.MustExec("set global tidb_auto_analyze_concurrency=1", Vec::new());
    let before_auto_succ = auto_succ();
    let before_auto_fail = auto_failed();
    assert!(domain.handle_auto_analyze());
    assert_eq!(auto_succ(), before_auto_succ + 1.0);
    assert_eq!(auto_failed(), before_auto_fail);
}

/// RAII 守卫：析构时恢复自动 ANALYZE 最小行数阈值，避免污染其他测试。
struct AutoAnalyzeMinCntGuard;

impl Drop for AutoAnalyzeMinCntGuard {
    fn drop(&mut self) {
        ResetAutoAnalyzeMinCnt();
    }
}
