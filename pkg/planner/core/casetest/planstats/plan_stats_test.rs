// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 本文件对应 pkg/planner/core/casetest/planstats/plan_stats_test.go，并通过真实
// testkit/Domain/统计缓存覆盖 ANALYZE、lease、同步/异步按需直方图加载、统计超时与
// pseudo 回退、计划缓存、Join/Apply/子查询/CTE、DDL 统计版本、虚拟列依赖，以及
// partial/allEvicted EXPLAIN 转换。

use astersql_domain::Domain;
use astersql_planner_core_rule as rule;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

/// `AsyncLoadHistogramNeededItems` is process-global, so tests that drain it
/// must not race each other under Rust's parallel test runner.
static ASYNC_HISTOGRAM_TEST_LOCK: Mutex<()> = Mutex::new(());

fn setup_common() {
    static SETUP: Once = Once::new();
    SETUP.call_once(astersql_testkit_testsetup::SetupForCommonTest);
}

/// 创建 mock store/Domain，建表 `t` 并插入 3 行，返回 `(domain, testkit, table_id)`。
fn setup_single_table() -> (Arc<Domain>, TestKit, i64) {
    setup_common();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table t(a int, b int, c int, d int, primary key(a), key idx(b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1,1,1,1),(2,2,2,2),(3,3,3,3)",
        Vec::new(),
    );
    let table_id = domain
        .table_by_name("test", "t")
        .expect("table t metadata")
        .ID;
    (domain, tk, table_id)
}

/// 对应 Go `countFullStats`：命中列 ID 返回直方图桶数+TopN 条数，未命中返回 -1。
///
/// `TableStats.columns` 直接以列 ID 为键，无需像 Go 那样线性遍历再比较 `col.Info.ID`。
// count_full_stats 对应 Go 的 countFullStats：命中列 ID 时返回
// Histogram.Len()+TopN.Num()，未命中保持 -1。`TableStats.columns` 直接以列 ID 为键，
// 因此不需要像 Go 那样线性遍历再比较 `col.Info.ID`。
fn count_full_stats(table_stats: &astersql_statistics_handle::TableStats, col_id: i64) -> i64 {
    table_stats
        .columns
        .get(&col_id)
        .map(|col| (col.buckets.len() + col.top_n.len()) as i64)
        .unwrap_or(-1)
}

/// 从 Domain 元数据按列名解析列 ID，并断言表 ID 与入参一致。
fn column_id(domain: &Domain, table_id: i64, column_name: &str) -> i64 {
    let table = domain.table_by_name("test", "t").expect("table t metadata");
    assert_eq!(table.ID, table_id);
    table
        .Columns
        .iter()
        .find(|column| column.Name.L == column_name)
        .unwrap_or_else(|| panic!("column {column_name} in table t"))
        .ID
}

/// 对应 Go TestPlanStatsLoad：`analyze ... all columns` 后各列 `countFullStats` >= 0。
// test_count_full_stats_matches_analyzed_columns 对应 Go TestPlanStatsLoad 里反复出现的
// `countFullStats(tbl.(...).TableInfo(), colID)` 用法：`analyze table ... all columns`
// 之后，每一个公开列都应该有完整（非空）的直方图/TopN，countFullStats 应返回 >= 0；
// 不存在的列 ID 应保持 -1。
#[test]
fn test_count_full_stats_matches_analyzed_columns() {
    let (domain, mut tk, table_id) = setup_single_table();
    tk.MustExec("analyze table t all columns", Vec::new());

    let handle = domain.stats_handle();
    let handle = handle.lock().expect("statistics handle");
    let table_stats = handle
        .stats_meta(table_id)
        .expect("cached table statistics after analyze");
    assert!(!table_stats.pseudo);

    for column_name in ["a", "b", "c", "d"] {
        let col_id = column_id(&domain, table_id, column_name);
        assert!(
            count_full_stats(table_stats, col_id) > 0,
            "column {column_name} should have full stats after `analyze table t all columns`"
        );
    }
    // 不存在的列 ID 必须保持 Go 版语义里的哨兵值 -1。
    assert_eq!(count_full_stats(table_stats, -1), -1);
}

/// 对应 Go TestPlanStatsLoad：临时改写 stats lease 后再精确还原（defer 语义）。
// test_stats_lease_round_trip_matches_go_defer_pattern 对应 Go TestPlanStatsLoad 开头的
// `originalLease := dom.StatsHandle().Lease(); dom.StatsHandle().SetLease(1); defer
// dom.StatsHandle().SetLease(originalLease)`：临时改写 lease 用于触发同步加载路径，
// 并在收尾时精确还原到原值。
#[test]
fn test_stats_lease_round_trip_matches_go_defer_pattern() {
    let (domain, _tk, _table_id) = setup_single_table();
    let handle = domain.stats_handle();

    let original_lease = handle.lock().expect("statistics handle").lease();
    domain
        .set_stats_lease(Duration::from_secs(1))
        .expect("set temporary lease");
    assert_eq!(
        handle.lock().expect("statistics handle").lease(),
        Duration::from_secs(1)
    );

    domain
        .set_stats_lease(original_lease)
        .expect("restore original lease");
    assert_eq!(
        handle.lock().expect("statistics handle").lease(),
        original_lease
    );
}

/// 对应 Go `LoadNeededHistograms`：无待加载项时重复调用须幂等且不改已加载统计。
// test_load_needed_histograms_is_idempotent_after_analyze 对应 Go TestPlanStatsLoad /
// TestPartialStatsInExplain 里反复调用的
// `dom.StatsHandle().LoadNeededHistograms(dom.InfoSchema())`：在没有待加载项目时，
// 重复调用必须是安全的幂等操作，不改变已经完整加载的统计信息，也不报错。
#[test]
fn test_load_needed_histograms_is_idempotent_after_analyze() {
    let _async_histogram_guard = ASYNC_HISTOGRAM_TEST_LOCK
        .lock()
        .expect("async histogram test lock");
    let (domain, mut tk, table_id) = setup_single_table();
    tk.MustExec("analyze table t all columns", Vec::new());
    domain
        .set_stats_lease(Duration::from_millis(1))
        .expect("enable lease-driven histogram reload path");

    let before = {
        let handle = domain.stats_handle();
        let handle = handle.lock().expect("statistics handle");
        handle
            .stats_meta(table_id)
            .expect("cached table statistics")
            .clone()
    };

    domain
        .load_needed_histograms()
        .expect("first LoadNeededHistograms call");
    domain
        .load_needed_histograms()
        .expect("second LoadNeededHistograms call must also succeed");

    let after = {
        let handle = domain.stats_handle();
        let handle = handle.lock().expect("statistics handle");
        handle
            .stats_meta(table_id)
            .expect("cached table statistics")
            .clone()
    };
    assert_eq!(before, after);
}

/// 对应 Go TestStatsAnalyzedInDDL：仅 DDL+analyze 推进 `mysql.stats_meta.version`。
// test_ddl_bumps_stats_meta_version_for_single_table 对应 Go TestStatsAnalyzedInDDL 的
// 核心断言：紧跟在 DDL 之后的下一次统计信息读取版本必须变化，而两次连续的 select 之间
// 版本保持不变。这里验证表级 `stats_meta.version`；Go 的逐索引 9 步 DDL/EXPLAIN
// 序列仍列在任务阻塞证据中，不能用本测试冒充完整覆盖。
#[test]
fn test_ddl_bumps_stats_meta_version_for_single_table() {
    let (domain, mut tk, table_id) = setup_single_table();
    tk.MustExec("analyze table t all columns", Vec::new());

    let version_query = format!("select version from mysql.stats_meta where table_id = {table_id}");
    let version_after_analyze = domain
        .restricted_stats_query(&version_query, &[])
        .expect("read stats_meta version after analyze");
    assert_eq!(version_after_analyze.len(), 1);

    // 两次连续 select（没有夹杂 DDL）不应改变已记录的统计版本。见文件头注释：
    // narrow runtime 对真实用户表的 SELECT 只做谓词列收集、不产生 record set（详见
    // `execute_relational_select`），因此这里用 `MustExec` 而不是 `MustQuery` 驱动它。
    tk.MustExec("select * from t where b > 1", Vec::new());
    tk.MustExec("select * from t where b > 1", Vec::new());
    let version_after_selects = domain
        .restricted_stats_query(&version_query, &[])
        .expect("read stats_meta version after selects");
    assert_eq!(version_after_analyze, version_after_selects);

    // DDL（加一个新索引并重新 analyze）之后版本必须变化。
    tk.MustExec("alter table t add index idx_c(c)", Vec::new());
    tk.MustExec("analyze table t all columns", Vec::new());
    let version_after_ddl = domain
        .restricted_stats_query(&version_query, &[])
        .expect("read stats_meta version after DDL");
    assert_ne!(version_after_selects, version_after_ddl);
}

/// 对应 Go TestCollectDependingVirtualCols 的 9 组真实表元数据场景。
#[test]
fn test_collect_depending_virtual_columns_from_table_metadata() {
    setup_common();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table t(a int, b int, c json, \
         index ic_char((cast(c->'$' as char(32) array))), \
         index ic_unsigned((cast(c->'$.unsigned' as unsigned array))), \
         index ic_signed((cast(c->'$.signed' as unsigned array))))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t1(a int, b int, c int, \
         vab int as (a + b) virtual, vc int as (c - 5) virtual, \
         vvc int as (b - vc) virtual, vvabvvc int as (vab * vvc) virtual, \
         index ib((b + 1)), index icvab((c + vab)), index ivvcvab((vvc / vab)))",
        Vec::new(),
    );

    let mut tables = BTreeMap::new();
    for table_name in ["t", "t1"] {
        let table = domain
            .table_by_name("test", table_name)
            .unwrap_or_else(|_| panic!("table {table_name} metadata"));
        tables.insert(table.ID, table.as_ref().clone());
    }
    let cases = [
        ("t", vec!["a", "b"], vec![]),
        (
            "t",
            vec!["c"],
            vec!["_v$_ic_char_0", "_v$_ic_signed_0", "_v$_ic_unsigned_0"],
        ),
        (
            "t",
            vec!["b", "c"],
            vec!["_v$_ic_char_0", "_v$_ic_signed_0", "_v$_ic_unsigned_0"],
        ),
        ("t1", vec!["a"], vec!["vab"]),
        ("t1", vec!["b"], vec!["_v$_ib_0", "vab", "vvc"]),
        ("t1", vec!["c"], vec!["_v$_icvab_0", "vc"]),
        (
            "t1",
            vec!["vab"],
            vec!["_v$_icvab_0", "_v$_ivvcvab_0", "vvabvvc"],
        ),
        (
            "t1",
            vec!["vab", "c"],
            vec!["_v$_icvab_0", "_v$_ivvcvab_0", "vc", "vvabvvc"],
        ),
        (
            "t1",
            vec!["vc", "c", "vvc"],
            vec!["_v$_icvab_0", "_v$_ivvcvab_0", "vvabvvc"],
        ),
    ];

    for (table_name, input_names, expected_names) in cases {
        let table = tables
            .values()
            .find(|table| table.Name.L == table_name)
            .unwrap_or_else(|| panic!("table {table_name}"));
        let needed = input_names
            .iter()
            .map(|name| {
                let column = table
                    .Columns
                    .iter()
                    .find(|column| column.Name.L == *name)
                    .unwrap_or_else(|| panic!("column {table_name}.{name}"));
                astersql_meta_model::StatsLoadItem {
                    TableItemID: astersql_meta_model::TableItemID {
                        TableID: table.ID,
                        ID: column.ID,
                        IsIndex: false,
                        IsSyncLoadFailed: false,
                    },
                    FullLoad: true,
                }
            })
            .collect::<Vec<_>>();
        let generated =
            rule::rule_collect_plan_stats::collect_depending_virtual_columns(&tables, &needed);
        let mut actual_names = generated
            .iter()
            .map(|item| {
                table
                    .Columns
                    .iter()
                    .find(|column| column.ID == item.TableItemID.ID)
                    .expect("generated column metadata")
                    .Name
                    .L
                    .clone()
            })
            .collect::<Vec<_>>();
        actual_names.sort();
        assert_eq!(actual_names, expected_names, "table {table_name}");
        assert!(generated.iter().all(|item| item.FullLoad));
    }
}

/// analyze 全列后：行数反映插入数据，各列具备完整直方图/TopN 统计。
// test_analyze_all_columns_marks_table_non_pseudo 对应 TestPlanStatsLoad 末尾
// full_scan_checks 里 `expect_pseudo: false` 的分支意图：真实执行过
// `analyze table ... all columns` 的表，其缓存统计不再是 pseudo。当前 Rust
// `Domain`/`statistics::handle` 实现在 DDL 建表时就会立即注册一条非 pseudo、
// `realtime_count = 0`、列统计尚未完整加载的 `TableStats`（与 Go 版"未 analyze 的表退化为
// pseudo 统计"的惰性策略不同，属于本任务 writes 清单之外的生产代码行为，这里如实反映而
// 不去改写生产逻辑）；因此断言的重点放在 Go 与 Rust 都认同的部分：`analyze table ... all
// columns` 之后，行数必须反映真实插入的数据，且每一列都必须从"未加载"变为有完整统计。
#[test]
fn test_analyze_all_columns_marks_table_non_pseudo() {
    let (domain, mut tk, table_id) = setup_single_table();
    {
        let handle = domain.stats_handle();
        let handle = handle.lock().expect("statistics handle");
        let before = handle
            .stats_meta(table_id)
            .expect("table registers a stats_meta row as soon as it is created");
        assert!(!before.pseudo);
        assert_eq!(before.realtime_count, 0);
        for column_name in ["a", "b", "c", "d"] {
            let col_id = column_id(&domain, table_id, column_name);
            assert!(
                count_full_stats(before, col_id) <= 0,
                "column {column_name} must not have full stats before analyze"
            );
        }
    }

    tk.MustExec("analyze table t all columns", Vec::new());

    let handle = domain.stats_handle();
    let handle = handle.lock().expect("statistics handle");
    let after = handle
        .stats_meta(table_id)
        .expect("cached table statistics after analyze");
    assert!(!after.pseudo);
    assert_eq!(after.realtime_count, 3);
    for column_name in ["a", "b", "c", "d"] {
        let col_id = column_id(&domain, table_id, column_name);
        assert!(
            count_full_stats(after, col_id) > 0,
            "column {column_name} should have full stats after `analyze table t all columns`"
        );
    }
}

/// 对应 Go `TestPartialStatsInExplain` 的首个同步加载往返：同步等待关闭时，
/// 首次 explain 必须显示 partial stats，显式加载直方图后该标记必须消失。
#[test]
fn test_partial_stats_explain_transitions_after_histogram_load() {
    let _async_histogram_guard = ASYNC_HISTOGRAM_TEST_LOCK
        .lock()
        .expect("async histogram test lock");
    setup_common();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    // `main_test::TestMain` is an ordinary Rust test and may run concurrently;
    // pin the Go case's dynamic-pruning default on this session explicitly.
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'dynamic'",
        Vec::new(),
    );
    tk.MustExec(
        "create table t(a int, b int, c int, primary key(a), key idx(b))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1,1,1),(2,2,2),(3,3,3)", Vec::new());
    tk.MustExec("create table t2(a int, primary key(a))", Vec::new());
    tk.MustExec("insert into t2 values (1),(2),(3)", Vec::new());
    tk.MustExec(
        "create table tp(a int, b int, c int, index ic(c)) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20), \
         partition p2 values less than maxvalue)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into tp values (1,1,1),(2,2,2),(13,13,13),(14,14,14),\
         (25,25,25),(36,36,36)",
        Vec::new(),
    );
    tk.MustExec("analyze table t all columns", Vec::new());
    tk.MustExec("analyze table t2 all columns", Vec::new());
    tk.MustExec("analyze table tp all columns", Vec::new());
    domain
        .set_stats_lease(Duration::from_secs(1))
        .expect("enable lease-driven histogram loading");
    // Rust ANALYZE publishes its full payload directly to the local cache,
    // whereas Go's non-zero-lease path observes the subsequent lite refresh.
    // Use the same clear + Update boundary as the repository's async-load
    // parity tests so this case exercises persisted lite membership rather
    // than the ANALYZE session's warm cache.
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .clear();
    domain
        .update_stats()
        .expect("reload lightweight statistics after analyze");
    tk.MustQuery("explain select * from tp where a = 1", Vec::new());
    tk.MustExec("set @@tidb_stats_load_sync_wait = 0", Vec::new());

    let cases: [(&str, &[&str], &[&str]); 5] = [
        (
            "explain format = brief select * from tp where b = 10",
            &["stats:partial[ic:unInitialized, b:unInitialized]"],
            &[],
        ),
        (
            "explain format = brief select * from tp where b = 10",
            &[],
            &["stats:partial["],
        ),
        (
            "explain format = brief select * from t join tp where tp.a = 10 and t.b = tp.c",
            &["stats:partial[", "allEvicted"],
            &[],
        ),
        (
            "explain format = brief select * from t join tp where tp.a = 10 and t.b = tp.c",
            &[],
            &["stats:partial["],
        ),
        (
            "explain format = brief select * from t join tp partition (p0) join t2 where t.a < 10 and t.b = tp.c and t2.a > 10 and t2.a = tp.c",
            &["IndexHashJoin", "stats:partial[", "allEvicted"],
            &[],
        ),
    ];
    for (sql, contains, not_contains) in cases {
        let plan = tk
            .MustQuery(sql, Vec::new())
            .Rows()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n");
        for expected in contains {
            assert!(
                plan.contains(expected),
                "sql={sql}, expected={expected}, plan={plan}"
            );
        }
        for unexpected in not_contains {
            assert!(
                !plan.contains(unexpected),
                "sql={sql}, unexpected={unexpected}, plan={plan}"
            );
        }
        domain
            .load_needed_histograms()
            .expect("LoadNeededHistograms after partial explain");
    }
}

/// 对应 Go `TestPlanStatsLoad` 的 DataSource 同步加载：lite cache 中只保留
/// 统计成员关系，优化带 `c > 1` 的查询时必须在返回计划前完整加载 c；未参与谓词的
/// b 仍保持未完整加载，证明不是把整表统计一次性灌回缓存来绕过按需加载。
#[test]
fn test_plan_stats_sync_loads_only_predicate_column() {
    let _async_histogram_guard = ASYNC_HISTOGRAM_TEST_LOCK
        .lock()
        .expect("async histogram test lock");
    let (domain, mut tk, table_id) = setup_single_table();
    tk.MustExec("analyze table t all columns", Vec::new());
    domain
        .set_stats_lease(Duration::from_millis(1))
        .expect("enable lease-driven histogram loading");
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .clear();
    domain
        .update_stats()
        .expect("reload lightweight statistics after analyze");

    let b_id = column_id(&domain, table_id, "b");
    let c_id = column_id(&domain, table_id, "c");
    {
        let handle = domain.stats_handle();
        let handle = handle.lock().expect("statistics handle");
        let lite = handle.stats_meta(table_id).expect("lite table statistics");
        assert!(count_full_stats(lite, b_id) <= 0);
        assert!(count_full_stats(lite, c_id) <= 0);
    }

    tk.MustExec(
        "set @@session.tidb_stats_load_sync_wait = 60000",
        Vec::new(),
    );
    tk.MustExec("select * from t where c > 1", Vec::new());

    let handle = domain.stats_handle();
    let handle = handle.lock().expect("statistics handle");
    let loaded = handle
        .stats_meta(table_id)
        .expect("table statistics after synchronous load");
    assert!(
        count_full_stats(loaded, c_id) > 0,
        "predicate column c must be synchronously full-loaded"
    );
    assert!(
        count_full_stats(loaded, b_id) <= 0,
        "unreferenced column b must remain evicted"
    );
}

/// 对应 Go `TestPlanStatsLoad` 的 Join/Apply/子查询/递归 CTE/索引矩阵。每个
/// case 都重新回到 lite cache，避免前一个查询已加载的统计掩盖后一个收集缺陷。
#[test]
fn test_plan_stats_sync_load_matrix() {
    let _guard = ASYNC_HISTOGRAM_TEST_LOCK.lock().unwrap();
    run_plan_stats_sync_load_matrix(0);
}

#[test]
fn test_plan_stats_sync_load_matrix_skips_failed_cases() {
    let _guard = ASYNC_HISTOGRAM_TEST_LOCK.lock().unwrap();
    assert_eq!(run_plan_stats_sync_load_matrix(1), 11);
}

#[test]
fn test_plan_stats_sync_load_matrix_rejects_all_failed_cases() {
    let _guard = ASYNC_HISTOGRAM_TEST_LOCK.lock().unwrap();
    let failure = std::panic::catch_unwind(|| run_plan_stats_sync_load_matrix(12))
        .expect_err("a matrix with no statistics assertions must fail");
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.contains("all cases failed sync stats loading"),
        "{message}"
    );
}

fn run_plan_stats_sync_load_matrix(forced_failures: usize) -> usize {
    setup_common();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'static'",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_stats_load_sync_wait = 60000",
        Vec::new(),
    );
    tk.MustExec(
        "create table t(a int, b int, c int, d int, primary key(a), key idx(b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1,1,1,1),(2,2,2,2),(3,3,3,3)",
        Vec::new(),
    );
    tk.MustExec("analyze table t all columns", Vec::new());
    tk.MustExec(
        "create table pt(a int, b int, c int) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20), \
         partition p2 values less than maxvalue)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into pt values (1,1,1),(2,2,2),(13,13,13),(14,14,14),\
         (25,25,25),(36,36,36)",
        Vec::new(),
    );
    tk.MustExec("analyze table pt all columns", Vec::new());
    domain
        .set_stats_lease(Duration::from_millis(1))
        .expect("enable lease-driven histogram loading");
    let table = domain.table_by_name("test", "t").expect("table t");

    let session = astersql_session::runtime::ConcreteSession::new(Arc::clone(&domain));
    session.execute("use test").unwrap();
    session
        .execute("set @@session.tidb_partition_prune_mode = 'static'")
        .unwrap();
    session
        .execute("set @@session.tidb_stats_load_sync_wait = 60000")
        .unwrap();
    struct RestorePseudo(bool);
    impl Drop for RestorePseudo {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(self.0);
        }
    }
    let _restore = RestorePseudo(astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Load());
    astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(true);
    let mut attempted_cases = 0;
    let mut checked_cases = 0;
    let mut explain = |sql: &str| {
        let _timeout = (attempted_cases < forced_failures).then(|| {
            astersql_testkit_testfailpoint::enable(
                "github.com/pingcap/tidb/pkg/statistics/handle/syncload/forceStatsSyncLoadTimeout",
                "return(true)",
            )
        });
        attempted_cases += 1;
        let mut results = session
            .execute(&format!("explain format = brief {sql}"))
            .expect("optimization must succeed even when sync loading uses pseudo fallback");
        let mut rows = Vec::new();
        while let Some(row) = results[0].next_row().unwrap() {
            rows.push(row);
        }
        // Match Go: only the statement's sync-load failure permits skipping.
        // A missing cache column without this flag must still fail assertions.
        let failed = session.WithSessionVars(|vars| {
            if vars.StmtCtx.IsSyncStatsFailed() {
                eprintln!(
                    "skip stats assertions for {sql:?} because sync stats load failed: {:?}",
                    vars.StmtCtx.GetWarnings()
                );
                true
            } else {
                false
            }
        });
        if failed { None } else { Some(rows) }
    };

    let cases = [
        ("data source", "select * from t where c>1", vec!["c"]),
        (
            "join",
            "select * from t t1 inner join t t2 on t1.b=t2.b where t1.d=3",
            vec!["d", "b"],
        ),
        (
            "apply",
            "select * from t t1 where t1.b > (select count(*) from t t2 where t2.c > t1.a and t2.d>1) and t1.c>2",
            vec!["c", "d"],
        ),
        (
            "any",
            "select * from t where t.b > any(select d from t where t.c > 2)",
            vec!["c"],
        ),
        (
            "in",
            "select * from t where t.b in (select d from t where t.c > 2)",
            vec!["c"],
        ),
        (
            "not in",
            "select * from t where t.b not in (select d from t where t.c > 2)",
            vec!["c"],
        ),
        (
            "exists",
            "select * from t t1 where exists (select * from t t2 where t1.b > t2.d and t2.c>1)",
            vec!["c"],
        ),
        (
            "not exists",
            "select * from t t1 where not exists (select * from t t2 where t1.b > t2.d and t2.c>1)",
            vec!["c"],
        ),
        (
            "recursive cte",
            "with recursive cte(x, y) as (select a, b from t where c > 1 union select x + 1, y from cte where x < 5) select * from cte",
            vec!["c"],
        ),
        (
            "non-recursive cte",
            "with cte(x, y) as (select d + 1, b from t where c > 1) select * from cte where x < 3",
            vec!["c"],
        ),
    ];
    for (name, sql, expected_columns) in cases {
        domain
            .stats_handle()
            .lock()
            .expect("statistics handle")
            .clear();
        domain.update_stats().expect("reload lite statistics");
        let Some(rows) = explain(sql) else {
            continue;
        };
        let plan = rows
            .iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        if name == "non-recursive cte" {
            assert_eq!(
                rows.into_iter()
                    .map(|row| row.join(" "))
                    .collect::<Vec<_>>(),
                vec![
                    "Projection 1.60 root  plus(test.t.d, 1)->Column#14, test.t.b",
                    "└─TableReader 1.60 root  data:Selection",
                    "  └─Selection 1.60 cop[tikv]  gt(test.t.c, 1), lt(plus(test.t.d, 1), 3)",
                    "    └─TableFullScan 3.00 cop[tikv] table:t keep order:false, stats:partial[idx:allEvicted]",
                ]
            );
        }
        let handle = domain.stats_handle();
        let handle = handle.lock().expect("statistics handle");
        let stats = handle.stats_meta(table.ID).expect("table statistics");
        for column_name in expected_columns {
            let column = table
                .Columns
                .iter()
                .find(|column| column.Name.L == column_name)
                .expect("column metadata");
            assert!(
                count_full_stats(stats, column.ID) > 0,
                "{name} must full-load predicate column {column_name}; loaded columns: {:?}; plan: {plan}",
                stats
                    .columns
                    .iter()
                    .map(|(id, column)| (*id, column.buckets.len(), column.top_n.len()))
                    .collect::<Vec<_>>()
            );
        }
        checked_cases += 1;
    }

    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .clear();
    domain.update_stats().expect("reload lite statistics");
    if explain("select * from t use index(idx) where b >= 10").is_some() {
        let handle = domain.stats_handle();
        let handle = handle.lock().expect("statistics handle");
        let stats = handle.stats_meta(table.ID).expect("table statistics");
        let index_id = table
            .Indices
            .iter()
            .find(|index| index.Name.L == "idx")
            .expect("idx metadata")
            .ID;
        assert!(
            stats
                .indexes
                .get(&index_id)
                .is_some_and(|index| index.fully_loaded),
            "USE INDEX(idx) must synchronously full-load idx"
        );

        checked_cases += 1;
    }
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .clear();
    domain.update_stats().expect("reload lite statistics");
    if explain("select * from pt where a < 15 and c > 1").is_some() {
        let partition_table = domain.table_by_name("test", "pt").expect("table pt");
        let c_id = partition_table
            .Columns
            .iter()
            .find(|column| column.Name.L == "c")
            .expect("pt.c")
            .ID;
        let handle = domain.stats_handle();
        let handle = handle.lock().expect("statistics handle");
        for partition in partition_table
            .GetPartitionInfo()
            .expect("partition metadata")
            .Definitions
            .iter()
            .filter(|partition| matches!(partition.Name.L.as_str(), "p0" | "p1"))
        {
            let stats = handle
                .stats_meta(partition.ID)
                .unwrap_or_else(|| panic!("partition {} statistics", partition.Name.L));
            assert!(
                count_full_stats(stats, c_id) > 0,
                "partition {} must full-load predicate column c",
                partition.Name.L
            );
        }
        checked_cases += 1;
    }
    assert!(checked_cases > 0, "all cases failed sync stats loading");
    checked_cases
}

/// 对应 Go `TestPlanStatsLoad` 的 issue #48257：单列全表扫描在同步加载后
/// 使用真实统计；异步加载完成前保持 pseudo，完成后切换为真实统计。
#[test]
fn test_issue_48257_full_scan_sync_and_async_stats_transition() {
    let _async_histogram_guard = ASYNC_HISTOGRAM_TEST_LOCK
        .lock()
        .expect("async histogram test lock");
    setup_common();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    domain
        .set_stats_lease(Duration::from_millis(1))
        .expect("enable lease-driven histogram loading");

    let explain = |tk: &mut TestKit, table: &str, expect_pseudo: bool| {
        let rows = tk
            .MustQuery(
                &format!("explain format = brief select * from {table}"),
                Vec::new(),
            )
            .Rows();
        assert_eq!(rows.len(), 2, "table={table}, rows={rows:?}");
        let plan = rows.into_iter().flatten().collect::<Vec<_>>().join("\n");
        for token in [
            "TableReader",
            "data:TableFullScan",
            "└─TableFullScan",
            &format!("table:{table}"),
        ] {
            assert!(
                plan.contains(token),
                "table={table}, token={token}, plan={plan}"
            );
        }
        assert_eq!(
            plan.contains("stats:pseudo"),
            expect_pseudo,
            "table={table}, plan={plan}"
        );
    };

    tk.MustExec(
        "set @@session.tidb_stats_load_sync_wait = 60000",
        Vec::new(),
    );
    tk.MustExec("create table t_issue48257(a int)", Vec::new());
    tk.MustExec("insert into t_issue48257 value(1)", Vec::new());
    tk.MustExec("analyze table t_issue48257 all columns", Vec::new());
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .clear();
    domain.update_stats().expect("reload lite sync statistics");
    explain(&mut tk, "t_issue48257", false);
    tk.MustExec("insert into t_issue48257 value(1)", Vec::new());
    domain
        .dump_stats_delta_to_kv(true)
        .expect("dump sync table delta");
    domain
        .update_stats()
        .expect("refresh sync table statistics");
    domain
        .load_needed_histograms()
        .expect("load sync table histogram");
    explain(&mut tk, "t_issue48257", false);
    tk.MustExec("set tidb_opt_objective='determinate'", Vec::new());
    explain(&mut tk, "t_issue48257", false);
    tk.MustExec("set tidb_opt_objective='moderate'", Vec::new());

    tk.MustExec("set @@session.tidb_stats_load_sync_wait = 0", Vec::new());
    tk.MustExec("create table t1_issue48257(a int)", Vec::new());
    tk.MustExec("insert into t1_issue48257 value(1)", Vec::new());
    tk.MustExec("analyze table t1_issue48257 all columns", Vec::new());
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .clear();
    domain.update_stats().expect("reload lite async statistics");
    explain(&mut tk, "t1_issue48257", true);
    tk.MustExec("insert into t1_issue48257 value(1)", Vec::new());
    domain
        .dump_stats_delta_to_kv(true)
        .expect("dump async table delta");
    domain
        .update_stats()
        .expect("refresh async table statistics");
    explain(&mut tk, "t1_issue48257", true);
    tk.MustExec("set tidb_opt_objective='determinate'", Vec::new());
    explain(&mut tk, "t1_issue48257", true);
    domain
        .load_needed_histograms()
        .expect("load async table histogram");
    explain(&mut tk, "t1_issue48257", false);
}

/// 对应 Go `TestPlanStatsLoadTimeout`：同步加载超时时，pseudo=false 必须令优化
/// 失败；pseudo=true 必须继续生成计划、标记同步失败并把 full-load 项转入异步队列。
#[test]
fn test_plan_stats_load_timeout_and_pseudo_fallback() {
    let _async_histogram_guard = ASYNC_HISTOGRAM_TEST_LOCK
        .lock()
        .expect("async histogram test lock");
    struct RestorePseudoTimeout(bool);
    impl Drop for RestorePseudoTimeout {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(self.0);
        }
    }

    let original = astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Load();
    let _restore = RestorePseudoTimeout(original);
    let _timeout = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/statistics/handle/syncload/forceStatsSyncLoadTimeout",
        "return(true)",
    );
    let (domain, mut tk, table_id) = setup_single_table();
    tk.MustExec("analyze table t all columns", Vec::new());
    domain
        .set_stats_lease(Duration::from_millis(1))
        .expect("enable sync load");
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .clear();
    domain.update_stats().expect("reload lite statistics");
    tk.MustExec("set @@session.tidb_stats_load_sync_wait = 1", Vec::new());

    astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(false);
    let error = tk.QueryToErr("explain format = brief select * from t where c > 1");
    assert!(error.message().contains("timeout"), "{error:?}");

    astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(true);
    tk.MustQuery(
        "explain format = brief select * from t where c > 1",
        Vec::new(),
    );
    let c_id = column_id(&domain, table_id, "c");
    assert!(
        astersql_statistics_asyncload::AsyncLoadHistogramNeededItems
            .AllItems()
            .iter()
            .any(|item| {
                item.TableItemID.TableID == table_id
                    && item.TableItemID.ID == c_id
                    && !item.TableItemID.IsIndex
                    && item.FullLoad
                    && item.TableItemID.IsSyncLoadFailed
            }),
        "pseudo fallback must retain a sync-failed async full-load item"
    );
}

/// 对应 Go `TestPreparedPlanCacheInvalidatedAfterSyncLoadTimeoutFallback`。
#[test]
fn test_prepared_plan_cache_recovers_after_sync_timeout_fallback() {
    let _async_histogram_guard = ASYNC_HISTOGRAM_TEST_LOCK
        .lock()
        .expect("async histogram test lock");
    struct RestorePseudoTimeout(bool);
    impl Drop for RestorePseudoTimeout {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(self.0);
        }
    }
    let _restore = RestorePseudoTimeout(astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Load());
    astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(true);
    let _timeout = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/statistics/handle/syncload/forceStatsSyncLoadTimeout",
        "return(true)",
    );

    setup_common();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "set @@session.tidb_enable_prepared_plan_cache = 1",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_plan_cache_invalidation_on_fresh_stats = 1",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_stats_load_sync_wait = 1", Vec::new());
    tk.MustExec("create table t(a int primary key, b int)", Vec::new());
    tk.MustExec(
        "insert into t values (1,1),(2,2),(3,3),(4,4),(5,5)",
        Vec::new(),
    );
    tk.MustExec("analyze table t all columns", Vec::new());
    domain
        .set_stats_lease(Duration::from_millis(1))
        .expect("enable sync load");
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .clear();
    domain.update_stats().expect("reload lite statistics");

    tk.MustExec("prepare st from 'select * from t where b > ?'", Vec::new());
    tk.MustExec("set @p = 2", Vec::new());
    tk.MustExec("execute st using @p", Vec::new());
    tk.MustQuery("select @@warning_count, @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1", "0"]]);
    tk.MustExec("execute st using @p", Vec::new());
    tk.MustQuery("select @@warning_count, @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1", "0"]]);

    domain
        .load_needed_histograms()
        .expect("async fallback histogram load");
    let table = domain.table_by_name("test", "t").expect("table t");
    let b_id = table
        .Columns
        .iter()
        .find(|column| column.Name.L == "b")
        .expect("t.b")
        .ID;
    assert!(
        domain
            .stats_handle()
            .lock()
            .expect("statistics handle")
            .stats_meta(table.ID)
            .and_then(|stats| stats.columns.get(&b_id))
            .is_some_and(|column| column.loaded_or_evicted),
        "async fallback must publish full b statistics before cache recovery"
    );
    tk.MustExec("execute st using @p", Vec::new());
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["0"]]);
    tk.MustExec("execute st using @p", Vec::new());
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);
}

/// 对应 Go `TestPlanStatsStatusRecord`：完整统计不记录状态；缓存降级为 allEvicted
/// 后，谓词列与关联索引都必须在语句上下文中留下 allEvicted 记录。
#[test]
fn test_plan_stats_status_records_all_evicted_items() {
    setup_common();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t (b int, key b(b))", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());
    tk.MustExec("analyze table t all columns", Vec::new());
    tk.MustQuery("select * from t where b >= 1", Vec::new());
    assert!(tk.Session().StatsLoadStatusesForTest().is_empty());

    let table = domain.table_by_name("test", "t").expect("table t");
    let column_id = table.Columns[0].ID;
    let index_id = table.Indices[0].ID;
    {
        let handle = domain.stats_handle();
        let mut handle = handle.lock().expect("statistics handle");
        let stats = handle
            .cache_mut()
            .get_mut(table.ID)
            .expect("cached table statistics");
        stats
            .columns
            .get_mut(&column_id)
            .expect("column statistics")
            .loaded_or_evicted = false;
        stats
            .indexes
            .get_mut(&index_id)
            .expect("index statistics")
            .fully_loaded = false;
    }
    tk.MustQuery("select * from t where b >= 1", Vec::new());
    let statuses = tk.Session().StatsLoadStatusesForTest();
    assert!(
        statuses
            .iter()
            .any(|(_, id, is_index, status)| *id == column_id
                && !*is_index
                && status == "allEvicted")
    );
    assert!(
        statuses
            .iter()
            .any(|(_, id, is_index, status)| *id == index_id
                && *is_index
                && status == "allEvicted")
    );
    assert!(
        statuses
            .iter()
            .all(|(_, _, _, status)| status == "allEvicted")
    );
}

/// 完整复现 Go `TestStatsAnalyzedInDDL` 的 9 步 DDL/EXPLAIN/直方图版本序列。
#[test]
fn test_stats_analyzed_during_ddl_nine_step_sequence() {
    setup_common();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set session tidb_stats_update_during_ddl = 1", Vec::new());
    tk.MustExec(
        "create table t(a int, b int, c int, primary key(a), key idx(b))",
        Vec::new(),
    );
    let values = (0..50)
        .map(|value| format!("({value},{value},{value})"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t values {values}"), Vec::new());

    let steps: [(&str, Option<(&str, &[&str])>); 9] = [
        ("alter table t add index idx_c(c)", None),
        (
            "select * from t use index(idx_c) where c > 1",
            Some((
                "idx_c",
                &[
                    "IndexLookUp 49.00 root  ",
                    "├─IndexRangeScan(Build) 49.00 cop[tikv] table:t, index:idx_c(c) range:(1,+inf], keep order:false",
                    "└─TableRowIDScan(Probe) 49.00 cop[tikv] table:t keep order:false",
                ],
            )),
        ),
        ("alter table t add index idx_bc(b,c)", None),
        (
            "select * from t use index(idx_bc) where b=1 and c <2",
            Some((
                "idx_bc",
                &[
                    "IndexReader 1.00 root  index:IndexRangeScan",
                    "└─IndexRangeScan 1.00 cop[tikv] table:t, index:idx_bc(b, c) range:[1 -inf,1 2), keep order:false",
                ],
            )),
        ),
        ("alter table t modify column b varchar(20)", None),
        (
            "select * from t use index(idx_bc) where b=1 and c <2",
            Some((
                "idx_bc",
                &[
                    "IndexReader 2.40 root  index:Selection",
                    "└─Selection 2.40 cop[tikv]  eq(cast(test.t.b, double BINARY), 1), lt(test.t.c, 2)",
                    "  └─IndexFullScan 50.00 cop[tikv] table:t, index:idx_bc(b, c) keep order:false",
                ],
            )),
        ),
        ("alter table t modify column c varchar(20)", None),
        (
            "select * from t use index(idx_c) where c > 1",
            Some((
                "idx_c",
                &[
                    "IndexLookUp 40.00 root  ",
                    "├─Selection(Build) 40.00 cop[tikv]  gt(cast(test.t.c, double BINARY), 1)",
                    "│ └─IndexFullScan 50.00 cop[tikv] table:t, index:idx_c(c) keep order:false",
                    "└─TableRowIDScan(Probe) 40.00 cop[tikv] table:t keep order:false",
                ],
            )),
        ),
        (
            "select * from t use index(idx_bc) where b=1 and c <2",
            Some((
                "idx_bc",
                &[
                    "IndexReader 40.00 root  index:Selection",
                    "└─Selection 40.00 cop[tikv]  eq(cast(test.t.b, double BINARY), 1), lt(cast(test.t.c, double BINARY), 2)",
                    "  └─IndexFullScan 50.00 cop[tikv] table:t, index:idx_bc(b, c) keep order:false",
                ],
            )),
        ),
    ];
    let mut last_was_select = false;
    let mut last_version: Option<String> = None;
    for (sql, select) in steps {
        let Some((index_name, expected_plan)) = select else {
            tk.MustExec(sql, Vec::new());
            last_was_select = false;
            continue;
        };
        let plan = tk
            .MustQuery(&format!("explain format = brief {sql}"), Vec::new())
            .Rows()
            .into_iter()
            .map(|row| row.join(" "))
            .collect::<Vec<_>>();
        assert_eq!(plan, expected_plan, "sql={sql}");
        let table = domain.table_by_name("test", "t").expect("table t");
        let index_id = table
            .Indices
            .iter()
            .find(|index| index.Name.L == index_name)
            .unwrap_or_else(|| panic!("index {index_name}"))
            .ID;
        let rows = domain
            .restricted_stats_query(
                &format!(
                    "select version from mysql.stats_histograms where table_id = {} and hist_id = {} and is_index = 1",
                    table.ID, index_id
                ),
                &[],
            )
            .expect("read index histogram version");
        assert_eq!(rows.len(), 1, "index {index_name} must be analyzed by DDL");
        let version = rows[0][0].clone();
        if let Some(previous) = last_version.as_ref() {
            if last_was_select {
                assert_eq!(&version, previous, "consecutive SELECT must keep version");
            } else {
                assert_ne!(&version, previous, "DDL must publish a new version");
            }
        }
        last_version = Some(version);
        last_was_select = true;
    }
}

/// A disabled loader must not consume the bounded queue on the planning thread.
#[test]
fn test_plan_stats_load_queue_without_workers_times_out() {
    let _guard = ASYNC_HISTOGRAM_TEST_LOCK
        .lock()
        .expect("async histogram test lock");
    struct RestoreConfig(astersql_config::Config, bool);
    impl Drop for RestoreConfig {
        fn drop(&mut self) {
            astersql_config::store_global_config(self.0.clone());
            astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(self.1);
        }
    }
    let old = astersql_config::get_global_config();
    let _restore = RestoreConfig(
        (*old).clone(),
        astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Load(),
    );
    let mut config = (*old).clone();
    config.performance.stats_load_concurrency = -1;
    config.performance.stats_load_queue_size = 1;
    astersql_config::store_global_config(config);
    astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(false);
    setup_common();
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table t(a int, b int, c int, primary key(a))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1,1,1),(2,2,2),(3,3,3)", Vec::new());
    tk.MustExec("analyze table t all columns", Vec::new());
    domain.set_stats_lease(Duration::from_millis(1)).unwrap();
    domain.stats_handle().lock().unwrap().clear();
    domain.update_stats().unwrap();
    tk.MustExec("set @@session.tidb_stats_load_sync_wait = 1", Vec::new());
    let error = tk.QueryToErr("select /*+ MAX_EXECUTION_TIME(1000) */ * from t where c>1");
    assert!(error.message().contains("sync load"), "{error:?}");
}

/// Direct planning must reset the same AST's context after previous SQLs,
/// including a sync-load failure, rather than skipping the next sync wait.
#[test]
fn test_plan_stats_load_full_queue_resets_direct_select_context() {
    use astersql_executor::select::ResetContextOfStmt;
    use astersql_parser_ast::NodeRef;
    use astersql_planner_core_base::Plan;
    use astersql_statistics_handle_syncload::{NeededItemTask, StatsLoadItem, TableItemID};
    use std::time::Instant;

    let _guard = ASYNC_HISTOGRAM_TEST_LOCK
        .lock()
        .expect("async histogram test lock");
    struct RestoreConfig(astersql_config::Config, bool);
    impl Drop for RestoreConfig {
        fn drop(&mut self) {
            astersql_config::store_global_config(self.0.clone());
            astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(self.1);
        }
    }
    let old = astersql_config::get_global_config();
    let _restore = RestoreConfig(
        (*old).clone(),
        astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Load(),
    );
    let mut config = (*old).clone();
    config.performance.stats_load_concurrency = -1;
    config.performance.stats_load_queue_size = 1;
    astersql_config::store_global_config(config);
    setup_common();
    let (domain, mut session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    session
        .execute("set @@session.tidb_analyze_version=2")
        .unwrap();
    session
        .execute("set @@session.tidb_stats_load_sync_wait = 1")
        .unwrap();
    session
        .execute("create table t(a int, b int, c int, primary key(a))")
        .unwrap();
    session
        .execute("insert into t values (1,1,1),(2,2,2),(3,3,3)")
        .unwrap();
    domain.set_stats_lease(Duration::from_nanos(1)).unwrap();
    session.execute("analyze table t all columns").unwrap();
    // Re-read the persisted ANALYZE statistics as lite metadata, as Go's
    // positive-lease StatsHandle.Update does before planning this SELECT.
    domain.stats_handle().lock().unwrap().clear();
    domain.update_stats().unwrap();
    let table = domain.table_by_name("test", "t").unwrap();
    let timeout = Duration::from_nanos(i64::MAX as u64);
    let (result_sender, _result_receiver) = std::sync::mpsc::sync_channel(1);
    session
        .AppendNeededStatsLoadItem(
            NeededItemTask {
                Item: StatsLoadItem {
                    TableItemID: TableItemID {
                        TableID: table.ID,
                        ID: table.Columns[0].ID,
                        IsIndex: false,
                    },
                    FullLoad: true,
                },
                ToTimeout: Instant::now() + timeout,
                ResultCh: result_sender,
                Retry: 0,
            },
            timeout,
        )
        .unwrap();
    let sql = "select /*+ MAX_EXECUTION_TIME(1000) */ * from t where c>1";
    let statement = NodeRef::new(
        astersql_parser::Parser::default()
            .ParseOneStmt(sql, "", "")
            .unwrap(),
    );

    session
        .execute("set global tidb_stats_load_pseudo_timeout=false")
        .unwrap();
    let mut value = session
        .execute("select @@tidb_stats_load_pseudo_timeout")
        .unwrap();
    assert_eq!(value[0].next_row().unwrap().unwrap(), vec!["0"]);
    drop(value);
    ResetContextOfStmt(&mut session, &statement).unwrap();
    session.WithSessionVars(|vars| {
        assert!(vars.StmtCtx.InSelectStmt);
        assert_eq!(vars.StmtCtx.StatsLoad.Timeout, Duration::ZERO);
        assert!(!vars.StmtCtx.IsSyncStatsFailed());
        assert_eq!(vars.StmtCtx.PendingStatsLoadItems(), 0);
    });
    assert_eq!(session.LastStatementHintsForTest().1, 1000);
    let error = match session.OptimizeParsedSelect(&statement) {
        Ok(plan) => panic!(
            "a full queue must fail direct planning when pseudo=false: wait={}, pending={}, failed={}, plan={}",
            plan.s_ctx()
                .GetSessionVars()
                .StatsLoadSyncWait
                .load(std::sync::atomic::Ordering::Acquire),
            plan.s_ctx()
                .GetSessionVars()
                .StmtCtx
                .PendingStatsLoadItems(),
            plan.s_ctx().GetSessionVars().StmtCtx.IsSyncStatsFailed(),
            plan.explain_info()
        ),
        Err(error) => error,
    };
    assert!(error.to_string().contains("sync load"), "{error}");
    session.WithSessionVars(|vars| assert!(vars.StmtCtx.IsSyncStatsFailed()));

    session
        .execute("set global tidb_stats_load_pseudo_timeout=true")
        .unwrap();
    let mut value = session
        .execute("select @@global.tidb_stats_load_pseudo_timeout")
        .unwrap();
    assert_eq!(value[0].next_row().unwrap().unwrap(), vec!["1"]);
    drop(value);
    for case in 0..2 {
        let _assertion = if case == 0 {
            astersql_testkit_testfailpoint::enable(
                "github.com/pingcap/executor/assertSyncStatsFailed",
                "return(true)",
            )
        } else {
            astersql_testkit_testfailpoint::enable(
                "github.com/pingcap/tidb/pkg/planner/core/assertSyncWaitFailed",
                "return(true)",
            )
        };
        let mut result = session.execute(sql).expect("pseudo SQL fallback");
        assert_eq!(result[0].next_row().unwrap().unwrap(), vec!["2", "2", "2"]);
        assert_eq!(result[0].next_row().unwrap().unwrap(), vec!["3", "3", "3"]);
        assert!(result[0].next_row().unwrap().is_none());
        // The two Go failpoints assert these exact states at their SQL
        // boundaries; inspect the real statement here instead of mocking them.
        session.WithSessionVars(|vars| {
            assert!(vars.StmtCtx.IsSyncStatsFailed());
            assert!(vars.StmtCtx.StatsSyncWaitError().is_some());
            assert_eq!(vars.StmtCtx.StatsLoad.Timeout, Duration::from_millis(1));
        });
    }
    ResetContextOfStmt(&mut session, &statement).unwrap();
    session.WithSessionVars(|vars| {
        assert!(!vars.StmtCtx.IsSyncStatsFailed());
        assert_eq!(vars.StmtCtx.PendingStatsLoadItems(), 0);
        assert_eq!(vars.StmtCtx.StatsLoad.Timeout, Duration::ZERO);
        assert!(vars.StmtCtx.StatsSyncWaitError().is_none());
    });
    assert_eq!(session.LastStatementHintsForTest().1, 1000);
    let plan = session
        .OptimizeParsedSelect(&statement)
        .expect("pseudo direct fallback");
    let reader = plan
        .as_any()
        .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalTableReader>()
        .expect("physical table reader");
    let hist = reader
        .stats_info()
        .HistColl
        .as_ref()
        .unwrap()
        .downcast_ref::<astersql_statistics::HistColl>()
        .expect("real histogram collection");
    for column in [&table.Columns[0], &table.Columns[2]] {
        let column_stats = hist
            .GetCol(column.ID)
            .expect("column metadata must be retained");
        assert_eq!(
            column_stats.Histogram.Len() + column_stats.TopN.as_ref().map_or(0, |topn| topn.Num()),
            0
        );
    }
    session.WithSessionVars(|vars| {
        assert!(
            vars.StmtCtx.IsSyncStatsFailed(),
            "reset must permit the second real sync wait"
        );
        assert!(vars.StmtCtx.StatsSyncWaitError().is_some());
    });
    drop(plan);
    session.execute("select 1").unwrap();
    session.WithSessionVars(|vars| {
        assert!(
            !vars.StmtCtx.IsSyncStatsFailed(),
            "the next SQL must not inherit the slow-log failure flag"
        );
        assert!(vars.StmtCtx.StatsSyncWaitError().is_none());
    });
}

/// Concurrent requests for one histogram share a load without consuming each
/// other's queue task. Both statements must receive the worker's result.
#[test]
fn test_plan_stats_load_shared_workers_complete_singleflight_followers() {
    use astersql_session::runtime::ConcreteSession;
    use std::sync::{Condvar, mpsc};
    let _guard = ASYNC_HISTOGRAM_TEST_LOCK.lock().unwrap();
    struct RestoreConfig(astersql_config::Config, bool);
    impl Drop for RestoreConfig {
        fn drop(&mut self) {
            astersql_config::store_global_config(self.0.clone());
            astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(self.1);
        }
    }
    let _restore = RestoreConfig(
        astersql_config::get_global_config().as_ref().clone(),
        astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Load(),
    );
    for concurrency in [2, 0] {
        let mut config = _restore.0.clone();
        config.performance.stats_load_concurrency = concurrency;
        config.performance.stats_load_queue_size = 1;
        astersql_config::store_global_config(config);
        astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Store(false);
        setup_common();
        let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
        session.execute("create table t(a int, c int)").unwrap();
        session
            .execute("insert into t values (1,1),(2,2),(3,3)")
            .unwrap();
        session
            .execute("set @@session.tidb_analyze_version = 2")
            .unwrap();
        session.execute("analyze table t all columns").unwrap();
        domain.set_stats_lease(Duration::from_millis(1)).unwrap();
        domain.stats_handle().lock().unwrap().clear();
        domain.update_stats().unwrap();
        let table = domain.table_by_name("test", "t").unwrap();
        let c_id = column_id(&domain, table.ID, "c");
        assert_eq!(
            count_full_stats(
                domain
                    .stats_handle()
                    .lock()
                    .unwrap()
                    .stats_meta(table.ID)
                    .unwrap(),
                c_id
            ),
            0
        );
        let sent = Arc::new((Mutex::new(0usize), Condvar::new()));
        let _requests = astersql_testkit_testfailpoint::enable_concurrent_call(
            "astersql/session/statsSyncLoadRequestsSent",
            {
                let sent = Arc::clone(&sent);
                move || {
                    *sent.0.lock().unwrap() += 1;
                    sent.1.notify_all();
                }
            },
        );
        let _read = astersql_testkit_testfailpoint::enable_concurrent_call(
            "astersql/session/statsSyncLoadBeforeRead",
            {
                let sent = Arc::clone(&sent);
                move || {
                    let (count, timeout) = sent
                        .1
                        .wait_timeout_while(
                            sent.0.lock().unwrap(),
                            Duration::from_secs(5),
                            |count| *count < 2,
                        )
                        .unwrap();
                    assert!(
                        !timeout.timed_out() && *count == 2,
                        "both singleflight requests must reach the queue before loading"
                    );
                }
            },
        );
        let (sender, receiver) = mpsc::channel();
        let mut callers = Vec::new();
        for _ in 0..2 {
            let domain = Arc::clone(&domain);
            let sender = sender.clone();
            callers.push(std::thread::spawn(move || {
                let session = ConcreteSession::new(domain);
                session.execute("use test").unwrap();
                session
                    .execute("set @@session.tidb_stats_load_sync_wait = 3000")
                    .unwrap();
                let rows = session
                    .execute("select c from t where c>1")
                    .and_then(|mut results| {
                        let mut rows = Vec::new();
                        let result = results.first_mut().expect("SELECT result");
                        while let Some(row) = result.next_row()? {
                            rows.push(row);
                        }
                        Ok(rows)
                    });
                sender.send(rows).unwrap();
            }));
        }
        drop(sender);
        for _ in 0..2 {
            let rows = receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("singleflight follower must finish without draining an empty queue")
                .unwrap();
            assert_eq!(rows, vec![vec!["2".to_owned()], vec!["3".to_owned()]]);
        }
        for caller in callers {
            caller.join().unwrap();
        }
        assert_eq!(*sent.0.lock().unwrap(), 2);
        assert!(
            count_full_stats(
                domain
                    .stats_handle()
                    .lock()
                    .unwrap()
                    .stats_meta(table.ID)
                    .unwrap(),
                c_id
            ) > 0
        );
    }
}
