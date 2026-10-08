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

// CTE（Common Table Expression，公用表表达式）执行器回归。
//
// 对应 Go `pkg/executor/test/cte/cte_test.go`，用真实 TestKit/MockStore 执行
// INSERT/SELECT/EXPLAIN、递归迭代、错误和 tracker 收尾路径。

/// 用内存 frontier/BTreeSet 模拟递归 CTE：插入成功才继续展开，重复边被去重。
#[test]
fn canonical_recursive_cte_uses_union_distinct_across_iterations() {
    use std::collections::BTreeSet;
    let mut frontier = vec![1_i64];
    let mut result = BTreeSet::new();
    // 弹出当前层值；仅当首次写入结果且未达上界时，再把下一层候选入队（故意 push 两次以模拟重复边）。
    while let Some(value) = frontier.pop() {
        if result.insert(value) && value < 5 {
            frontier.push(value + 1);
            frontier.push(value + 1);
        }
    }
    assert_eq!(result.into_iter().collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
}

#[test]
fn test_cte_issue_49096_executes_insert_select_without_deadlock() {
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t1, t2", Vec::new());
    tk.MustExec("create table t1(c1 int)", Vec::new());
    tk.MustExec("create table t2(c1 int)", Vec::new());
    let _close_panic = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/mock_cte_exec_panic_avoid_deadlock",
        "return(true)",
    );
    tk.MustExec(
        "insert into t1 values (0), (1), (2), (3), (4), (5), (6), (7), (8), (9)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t2 with cte1 as (select c1 from t1) select c1 from cte1 natural join (select * from cte1 where c1 > 0) cte2 order by c1",
        Vec::new(),
    );
}

#[test]
fn test_spill_to_disk_preserves_recursive_union_distinct_results() {
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t1", Vec::new());
    tk.MustExec("create table t1(c1 int)", Vec::new());

    let values = (0..1000)
        .map(|value| format!("({})", value % 100))
        .collect::<Vec<_>>()
        .join(", ");
    tk.MustExec(&format!("insert into t1 values {values}"), Vec::new());
    tk.MustExec("set cte_max_recursion_depth = 500000", Vec::new());
    let _spill = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/testCTEStorageSpill",
        "return(true)",
    );

    let rows = tk
        .MustQuery(
            "with recursive cte1 as (select c1 from t1 union select c1 + 1 c1 from cte1 where c1 < 1000) select c1 from cte1 order by c1",
            Vec::new(),
        )
        .Rows();
    let expected = (0..=1000)
        .map(|value| vec![value.to_string()])
        .collect::<Vec<_>>();
    assert_eq!(rows, expected);
    assert!(
        tk.Session().GetSessionVars().MemTracker().MaxConsumed() > 0,
        "recursive CTE execution must charge the statement memory tracker"
    );
    assert!(
        tk.Session().GetSessionVars().DiskTracker().MaxConsumed() > 0,
        "forced recursive CTE spill must charge the statement disk tracker"
    );
}

#[test]
fn test_cte_exec_error_reports_integer_overflow() {
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists src", Vec::new());
    tk.MustExec("create table src(first int, second int)", Vec::new());
    // Go uses 1001 random pairs in [0, 1000).  Use the deterministic upper
    // bound so the same Fibonacci recurrence necessarily reaches BIGINT
    // overflow before iteration 80 without changing the recursive query.
    let values = (0..=1000)
        .map(|_| "(999, 999)")
        .collect::<Vec<_>>()
        .join(", ");
    tk.MustExec(&format!("insert into src values {values}"), Vec::new());
    tk.MustExec("set tidb_max_chunk_size = 32", Vec::new());
    tk.MustExec("set tidb_projection_concurrency = 20", Vec::new());
    let _spill = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/testCTEStorageSpill",
        "return(true)",
    );

    let sql = "with recursive cte(iter, first, second, result) as (select 1, first, second, first+second from src union all select iter+1, second, result, second+result from cte where iter < 80) select * from cte";
    for _ in 0..10 {
        let error = tk.QueryToErr(sql);
        assert!(
            error
                .message()
                .to_ascii_lowercase()
                .contains("integer overflow"),
            "expected overflow error, got {error}"
        );
    }
}

#[test]
fn test_cte_panics_are_recovered_as_query_errors() {
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t1(c1 int)", Vec::new());
    tk.MustExec("insert into t1 values (1), (2), (3)", Vec::new());

    let sql = "with recursive cte1 as (select c1 from t1 union all select c1 + 1 from cte1 where c1 < 5) select t_alias_1.c1 from cte1 as t_alias_1 inner join cte1 as t_alias_2 on t_alias_1.c1 = t_alias_2.c1 order by c1";
    for failpoint in ["testCTESeedPanic", "testCTERecursivePanic"] {
        let full_name = format!("github.com/pingcap/tidb/pkg/executor/{failpoint}");
        let guard =
            astersql_testkit_testfailpoint::enable(&full_name, &format!("panic(\"{failpoint}\")"));
        let error = tk.QueryToErr(sql);
        assert!(
            error.message().contains(failpoint),
            "expected recovered {failpoint}, got {error}"
        );
        drop(guard);
    }
}

#[test]
fn test_cte_recursive_limit_error_is_recovered_and_reusable() {
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set cte_max_recursion_depth = 2", Vec::new());
    let error = tk.QueryToErr(
        "with recursive cte1 as (select 1 as c1 union all select c1 + 1 from cte1 where c1 < 5) select c1 from cte1",
    );
    assert!(error.message().contains("recursive"));

    tk.MustExec("set cte_max_recursion_depth = 100", Vec::new());
    tk.MustQuery(
        "with recursive cte1 as (select 1 as c1 union all select c1 + 1 from cte1 where c1 < 5) select c1 from cte1 order by c1",
        Vec::new(),
    )
    .Check(astersql_testkit::Rows(&["1", "2", "3", "4", "5"]));
}

#[test]
fn test_cte_del_spill_file_cleans_statement_state_after_error() {
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t1, t2", Vec::new());
    tk.MustExec("create table t1(c1 int, c2 int)", Vec::new());
    tk.MustExec("create table t2(c1 int)", Vec::new());
    tk.MustExec("insert into t2 values (1)", Vec::new());
    tk.MustExec("set cte_max_recursion_depth = 100", Vec::new());
    tk.MustExec("set tidb_mem_quota_query = 100", Vec::new());
    tk.MustExec("set global tidb_mem_oom_action = 'cancel'", Vec::new());

    let sql = "insert into t1 (c1, c2) with recursive cte1 as (select c1 from t2 union select cte1.c1 + 1 from cte1 where cte1.c1 < 100) select cte1.c1, cte1.c1+1 from cte1";
    tk.MustExecToErr(sql);
    tk.MustExec("set global tidb_mem_oom_action = 'log'", Vec::new());
    tk.MustExec(sql, Vec::new());
    assert!(
        tk.Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .is_empty(),
        "failed CTE execution must release statement trackers"
    );
    assert!(
        tk.Session().GetSessionVars().CTEStorageMapIsEmpty(),
        "completed CTE execution must clear statement CTE storage"
    );
}

#[test]
fn test_cte_share_correlated_column_with_both_hash_join_build_sides() {
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t1, t2", Vec::new());
    tk.MustExec("create table t1(c1 int, c2 varchar(100))", Vec::new());
    tk.MustExec("insert into t1 values (1, '2020-10-10')", Vec::new());
    tk.MustExec("create table t2(c1 int, c2 date)", Vec::new());
    tk.MustExec("insert into t2 values (1, '2020-10-10')", Vec::new());

    for hint in ["alias1", "alias2"] {
        let sql = format!(
            "with cte1 as (select t1.c1, (select t2.c2 from t2 where t2.c2 = str_to_date(t1.c2, '%Y-%m-%d')) as c2 from t1 inner join t2 on t1.c1 = t2.c1) select /*+ hash_join_build({hint}) */ * from cte1 alias1 inner join cte1 alias2 on alias1.c1 = alias2.c1"
        );
        for _ in 0..100 {
            tk.MustQuery(&sql, Vec::new())
                .Check(astersql_testkit::Rows(&["1 2020-10-10 1 2020-10-10"]));
        }
    }

    tk.MustExec("drop table if exists t1", Vec::new());
    tk.MustExec("create table t1(a int)", Vec::new());
    tk.MustExec("insert into t1 values (1), (2)", Vec::new());
    tk.MustQuery(
        "select * from t1 dt where exists (with recursive qn as (select a as b union all select b + 1 from qn where b = 0 or b = 1) select * from qn dtqn1 where exists (select /*+ no_decorrelate() */ b from qn where dtqn1.b + 1))",
        Vec::new(),
    )
    .Check(astersql_testkit::Rows(&["1", "2"]));
}

#[test]
fn test_cte_iteration_memory_tracker_is_released_after_explain_analyze() {
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t1", Vec::new());
    tk.MustExec("create table t1(c1 int)", Vec::new());
    tk.MustExec("insert into t1 values (0), (1), (2)", Vec::new());
    tk.MustExec("set cte_max_recursion_depth = 100", Vec::new());
    tk.MustExec("set tidb_mem_quota_query = 10", Vec::new());
    tk.MustQuery("select @@tidb_mem_quota_query", Vec::new())
        .Check(astersql_testkit::Rows(&["10"]));
    let _assert_spill = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/assertIterTableSpillToDisk",
        "return(20)",
    );

    let rows = tk
        .MustQuery(
            "explain analyze with recursive cte1 as (select c1 from t1 union all select c1 + 1 c1 from cte1 where c1 < 20) select * from cte1",
            Vec::new(),
        )
        .Rows();
    assert!(rows.iter().flatten().any(|cell| cell.contains("CTE")));
    assert!(
        tk.Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .is_empty(),
        "explain analyze must release CTE iteration trackers"
    );
    assert!(
        tk.Session().GetSessionVars().DiskTracker().MaxConsumed() > 0,
        "the 10-byte quota must spill recursive CTE materialization to disk"
    );
}

#[test]
fn test_cte_table_invalid_task_has_recursive_plan_without_warnings() {
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table p (groupid bigint default null, key k1 (groupid))",
        Vec::new(),
    );
    tk.MustExec(
        "create table g (groupid bigint default null, parentid bigint not null, key k1 (parentid), key k2 (groupid, parentid))",
        Vec::new(),
    );
    tk.MustExec("set tidb_opt_enable_hash_join = off", Vec::new());
    let expected_plan = [
        "Projection 9990.00 root  1->Column#22",
        "└─IndexJoin 9990.00 root  inner join, inner:IndexReader, outer key:test.p.groupid, inner key:test.g.groupid, equal cond:eq(test.p.groupid, test.g.groupid)",
        "  ├─HashAgg(Build) 12800.00 root  group by:test.p.groupid, funcs:firstrow(test.p.groupid)->test.p.groupid",
        "  │ └─Selection 12800.00 root  not(isnull(test.p.groupid))",
        "  │   └─CTEFullScan 16000.00 root CTE:w data:CTE_0",
        "  └─IndexReader(Probe) 9990.00 root  index:Selection",
        "    └─Selection 9990.00 cop[tikv]  not(isnull(test.g.groupid))",
        "      └─IndexRangeScan 10000.00 cop[tikv] table:g, index:k2(groupid, parentid) range: decided by [eq(test.g.groupid, test.p.groupid)], keep order:false, stats:pseudo",
        "CTE_0 16000.00 root  Recursive CTE",
        "├─IndexReader(Seed Part) 10000.00 root  index:IndexFullScan",
        "│ └─IndexFullScan 10000.00 cop[tikv] table:p, index:k1(groupid) keep order:false, stats:pseudo",
        "└─IndexHashJoin(Recursive Part) 10000.00 root  inner join, inner:IndexLookUp, outer key:test.p.groupid, inner key:test.g.parentid, equal cond:eq(test.p.groupid, test.g.parentid)",
        "  ├─Selection(Build) 8000.00 root  not(isnull(test.p.groupid))",
        "  │ └─CTETable 10000.00 root  Scan on CTE_0",
        "  └─IndexLookUp(Probe) 10000.00 root  ",
        "    ├─IndexRangeScan(Build) 10000.00 cop[tikv] table:g, index:k1(parentid) range: decided by [eq(test.g.parentid, test.p.groupid)], keep order:false, stats:pseudo",
        "    └─TableRowIDScan(Probe) 10000.00 cop[tikv] table:g keep order:false, stats:pseudo",
    ]
    .into_iter()
    .map(|line| line.splitn(4, ' ').map(str::to_owned).collect::<Vec<_>>())
    .collect::<Vec<_>>();
    tk.MustQuery(
        "explain format='brief' with recursive w(gid) as (select groupid from p union select g.groupid from g join w on g.parentid = w.gid) select 1 from g where g.groupid in (select gid from w)",
        Vec::new(),
    )
    .Check(expected_plan);
    tk.MustQuery("show warnings", Vec::new())
        .Check(astersql_testkit::Rows(&[]));
}
