// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 并行 Apply 端到端回归测试。
//
// 与 parallel_apply_test.go 一致，全部通过真实 testkit 会话建表、规划和执行；
// EXPLAIN/EXPLAIN ANALYZE 的断言只消费运行时返回的计划，不构造固定计划文本。

#![allow(non_snake_case)]

use astersql_sessionctx_vardef::{DefTiDBEnableParallelApply, TiDBEnableParallelApply};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

fn new_testkit() -> TestKit {
    let (store, _) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("use test", Vec::new());
    testkit
}

fn explain_lines(testkit: &TestKit, sql: &str) -> Vec<String> {
    testkit
        .MustQuery(sql, Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row.join(" "))
        .collect()
}

fn plan_tree_rows(lines: &[&str]) -> Vec<Vec<String>> {
    lines
        .iter()
        .map(|line| line.splitn(4, ' ').map(str::to_owned).collect())
        .collect()
}

fn contains_apply_concurrency_gt_one(rows: &[String]) -> bool {
    for row in rows {
        if !(row.contains("Apply") && row.contains("Concurrency:")) {
            continue;
        }
        return row
            .split("Concurrency:")
            .nth(1)
            .and_then(|rest| rest.trim_start().split_whitespace().next())
            .and_then(|value| value.parse::<i32>().ok())
            .is_some_and(|value| value > 1);
    }
    false
}

fn assert_plan_has_apply_and_keep_order(testkit: &TestKit, sql: &str, expect_keep_order: bool) {
    let rows = explain_lines(testkit, &format!("explain format = 'plan_tree' {sql}"));
    let mut found_apply = false;
    let mut found_keep_order = false;
    let mut in_build_side = false;
    for line in &rows {
        if line.contains("Apply") {
            found_apply = true;
        }
        if line.contains("Build") {
            in_build_side = true;
        } else if line.contains("Probe") {
            in_build_side = false;
        }
        if in_build_side && line.contains("keep order:true") {
            found_keep_order = true;
        }
    }
    assert!(
        found_apply,
        "plan should contain Apply: sql={sql:?}, rows={rows:?}"
    );
    if expect_keep_order {
        assert!(
            found_keep_order,
            "outer Build side should keep order: sql={sql:?}, rows={rows:?}"
        );
    }
}

#[test]
fn test_lateral_hierarchy_parallel_apply() {
    assert_eq!(TiDBEnableParallelApply, "tidb_enable_parallel_apply");
    assert!(!DefTiDBEnableParallelApply);

    let mut testkit = new_testkit();
    testkit.MustExec(
        "create table category (
            id int primary key, parent_id int, name varchar(50), sort_order int,
            index idx_parent(parent_id, sort_order, name))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into category values
            (1, null, 'root', 0),
            (2, 1, 'child_a', 1), (3, 1, 'child_b', 2), (4, 1, 'child_c', 3), (5, 1, 'child_d', 4),
            (6, 2, 'grandchild_a1', 1), (7, 2, 'grandchild_a2', 2), (8, 2, 'grandchild_a3', 3),
            (9, 3, 'grandchild_b1', 1), (10, 3, 'grandchild_b2', 2)",
        Vec::new(),
    );
    testkit.MustExec("set tidb_enable_parallel_apply=on", Vec::new());
    testkit.MustExec("set tidb_executor_concurrency=5", Vec::new());

    let hierarchy_sql = r#"with recursive tree as (
            select id, parent_id, name, 1 as depth from category where parent_id is null
            union all
            select c.id, c.parent_id, c.name, tree.depth + 1
            from tree cross join lateral (
                select id, parent_id, name from category
                where parent_id = tree.id
                order by parent_id, sort_order limit 2
            ) as c
            where tree.depth < 3
        ) select id, name, depth from tree order by depth, id, name"#;

    let explain_hierarchy_sql = format!("explain format='plan_tree' {hierarchy_sql}");
    let plan_rows = explain_lines(&testkit, &explain_hierarchy_sql);
    assert!(
        plan_rows.iter().any(|line| line.contains("Apply")),
        "plan must contain Apply: {plan_rows:?}"
    );

    let flat_sql = r#"select p.id, c.id as child_id from category p cross join lateral (
            select id from category where parent_id = p.id order by sort_order limit 2
        ) as c where p.parent_id is null"#;
    let analyze_rows = explain_lines(&testkit, &format!("explain analyze {flat_sql}"));
    assert!(
        contains_apply_concurrency_gt_one(&analyze_rows),
        "EXPLAIN ANALYZE must report Concurrency > 1: {analyze_rows:?}"
    );

    testkit.MustExec("set tidb_enable_parallel_apply=off", Vec::new());
    let serial_rows = testkit.MustQuery(hierarchy_sql, Vec::new()).Rows();
    testkit.MustExec("set tidb_enable_parallel_apply=on", Vec::new());
    let parallel_rows = testkit.MustQuery(hierarchy_sql, Vec::new()).Rows();
    assert_eq!(
        serial_rows, parallel_rows,
        "recursive CTE + LATERAL must be independent of parallel_apply"
    );
}

fn run_parallel_apply_warning_case(cascades: &str) {
    let mut testkit = new_testkit();
    testkit.MustExec(
        &format!("set tidb_enable_cascades_planner={cascades}"),
        Vec::new(),
    );
    testkit.MustExec("create table t1 (a int, b int, c int)", Vec::new());
    testkit.MustExec("create table t2 (a int, b int, c int, key(a))", Vec::new());
    testkit.MustExec("create table t3(a int, b int, c int, key(a))", Vec::new());
    testkit.MustExec("set tidb_enable_parallel_apply=on", Vec::new());
    testkit.MustQuery(
        "select (select /*+ inl_hash_join(t2, t3) */ 1 from t2, t3 \
         where t2.a=t3.a and t2.b > t1.b) from t1",
        Vec::new(),
    );
    testkit
        .MustQuery("show warnings", Vec::new())
        .Check(Rows(&[]));

    testkit.MustExec("create table t(a int, b int, index idx(a))", Vec::new());
    testkit
        .MustQuery(
            "explain format = 'plan_tree' select t3.a from t t3 \
             where (select /*+ inl_join(t1) */ count(*) from t t1 join t t2 \
             on t1.a=t2.a and t1.b>t3.b)",
            Vec::new(),
        )
        .Check(plan_tree_rows(&[
            "Projection root  test.t.a",
            "└─Apply root  CARTESIAN inner join",
            "  ├─TableReader(Build) root  data:TableFullScan",
            "  │ └─TableFullScan cop[tikv] table:t3 keep order:false, stats:pseudo",
            "  └─Selection(Probe) root  Column",
            "    └─HashAgg root  funcs:count(1)->Column",
            "      └─IndexJoin root  inner join, inner:IndexLookUp, outer key:test.t.a, inner key:test.t.a, equal cond:eq(test.t.a, test.t.a)",
            "        ├─IndexReader(Build) root  index:IndexFullScan",
            "        │ └─IndexFullScan cop[tikv] table:t2, index:idx(a) keep order:false, stats:pseudo",
            "        └─IndexLookUp(Probe) root  ",
            "          ├─Selection(Build) cop[tikv]  not(isnull(test.t.a))",
            "          │ └─IndexRangeScan cop[tikv] table:t1, index:idx(a) range: decided by [eq(test.t.a, test.t.a)], keep order:false, stats:pseudo",
            "          └─Selection(Probe) cop[tikv]  gt(test.t.b, test.t.b)",
            "            └─TableRowIDScan cop[tikv] table:t1 keep order:false, stats:pseudo",
        ]));
    testkit
        .MustQuery("show warnings", Vec::new())
        .Check(Rows(&[]));
}

#[test]
fn test_parallel_apply_warnning() {
    run_parallel_apply_warning_case("off");
    run_parallel_apply_warning_case("on");
}

#[test]
fn test_parallel_apply_ordered_plan() {
    let mut testkit = new_testkit();
    testkit.MustExec("create table t1 (a int, b int, index idx_a(a))", Vec::new());
    testkit.MustExec("create table t2 (a int, b int)", Vec::new());
    testkit.MustExec("set tidb_enable_parallel_apply=on", Vec::new());
    testkit.MustExec("set tidb_executor_concurrency=5", Vec::new());

    assert_plan_has_apply_and_keep_order(
        &testkit,
        "select t1.a, (select max(t2.b) from t2 where t2.a <= t1.a) \
         from t1 order by t1.a",
        true,
    );
    assert_plan_has_apply_and_keep_order(
        &testkit,
        "select t1.a, (select max(t2.b) from t2 where t2.a <= t1.a) \
         from t1 order by t1.a limit 5",
        true,
    );
    assert_plan_has_apply_and_keep_order(
        &testkit,
        "select t1.a, (select max(t2.b) from t2 where t2.a <= t1.a) from t1",
        false,
    );
    assert_plan_has_apply_and_keep_order(
        &testkit,
        "select t1.a from t1 where exists \
         (select /*+ NO_DECORRELATE() */ 1 from t2 where t2.a = t1.a) \
         order by t1.a limit 3",
        true,
    );

    explain_lines(
        &testkit,
        "explain format = 'plan_tree' select t1.a, \
         (select max(t2.b) from t2 where t2.a <= t1.a) from t1 order by t1.a",
    );
    testkit
        .MustQuery("show warnings", Vec::new())
        .Check(Rows(&[]));

    testkit.MustExec(
        "insert into t1 values (1,10),(2,20),(3,30),(4,40),(5,50)",
        Vec::new(),
    );
    testkit.MustExec("insert into t2 values (1,1),(2,2),(3,3)", Vec::new());

    let result_sql = "select t1.a, (select max(t2.b) from t2 where t2.a <= t1.a) \
                      from t1 order by t1.a limit 3";
    testkit.MustExec("set tidb_enable_parallel_apply=off", Vec::new());
    let serial_rows = testkit.MustQuery(result_sql, Vec::new()).Rows();
    testkit.MustExec("set tidb_enable_parallel_apply=on", Vec::new());
    let parallel_rows = testkit.MustQuery(result_sql, Vec::new()).Rows();
    assert_eq!(serial_rows, parallel_rows);
}
