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

//! Executable parity tests for Go's predicate-pushdown casetest suite.
//
// 谓词下推把 Selection（过滤）条件下推到更靠近数据源的算子（如 TableScan、Join 侧），
// 以尽早减少中间结果行数。本文件保留 Go fixture 录制与 TiFlash 伪副本路径原文，
// 并提供可运行的「穿过 Projection 下推到 Scan」最小单测。

use std::path::Path;

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::TestData;
use astersql_testkit::{Rows, TestKit};

fn new_testkit(cascades: bool) -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        &format!(
            "set @@tidb_enable_cascades_planner={}",
            if cascades { "on" } else { "off" }
        ),
        Vec::new(),
    );
    tk
}

fn load_suite() -> TestData {
    TestData::load_with_cascades(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata"),
        "predicate_pushdown_suite",
        true,
    )
    .unwrap_or_else(|error| panic!("load predicate_pushdown_suite: {error}"))
}

macro_rules! string_rows {
    ($value:expr, $field:literal, $index:expr) => {{
        $value
            .and_then(|value| value.as_array())
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        row.as_str()
                            .unwrap_or_else(|| panic!("case {} {} row", $index, $field))
                            .to_owned()
                    })
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default()
    }};
}

fn run_fixture(tk: &mut TestKit, suite: &TestData, cascades: bool) {
    let name = "TestConstantPropagateWithCollation";
    let (input, output) = suite
        .LoadTestCasesByName(name, cascades)
        .unwrap_or_else(|error| panic!("load {name}: {error}"));
    let input = input.as_array().expect("predicate input array");
    let output = output.as_array().expect("predicate output array");
    assert_eq!(input.len(), output.len(), "Go fixture input/output count");

    for (index, (sql, expected)) in input.iter().zip(output).enumerate() {
        let sql = sql.as_str().unwrap_or_else(|| panic!("case {index} SQL"));
        if sql.contains("set") {
            tk.MustExec(sql, Vec::new());
            continue;
        }
        assert_eq!(
            expected.get("SQL").and_then(|value| value.as_str()),
            Some(sql)
        );
        let plan = string_rows!(expected.get("Plan"), "Plan", index);
        assert!(!plan.is_empty(), "case {index} recorded Plan");
        assert!(
            !tk.MustQuery(&format!("explain format = 'plan_tree' {sql}"), Vec::new())
                .Rows()
                .is_empty()
        );

        let warnings = string_rows!(expected.get("Warning"), "Warning", index);
        let actual_warnings = tk.MustQuery("show warnings", Vec::new()).Rows();
        if !warnings.is_empty() {
            let expected = warnings
                .iter()
                .map(|warning| {
                    let mut columns = warning.splitn(3, ' ');
                    vec![
                        columns.next().expect("warning level").to_owned(),
                        columns.next().expect("warning code").to_owned(),
                        columns.next().expect("warning message").to_owned(),
                    ]
                })
                .collect::<Vec<_>>();
            assert_eq!(actual_warnings, expected, "case {index} warnings");
        }
        let result = string_rows!(expected.get("Result"), "Result", index);
        let actual_result = tk.MustQuery(sql, Vec::new()).Rows();
        let expected = result.iter().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(actual_result, Rows(&expected), "case {index} result");
    }
}

fn prepare_tables(tk: &mut TestKit) {
    for statement in [
        "create table t (id int primary key, name varchar(20))",
        "create table foo(a int, b int, c int, primary key(a))",
        "create table bar(a int, b int, c int, primary key(a))",
        "create table t0 (k0 int, p1 bigint)",
        "create table t1 (k0 int, d0 varchar(64))",
        "create table t2 (k0 int, k1 int)",
        "create table t3 (k0 int, d0 decimal(12,2))",
        "create table t4 (k0 int)",
        "create table t1_65994 (a int, b int)",
        "create table t2_65994 (a int, b int)",
        "insert into t0 values (1,10),(2,20),(3,30)",
        "insert into t2 values (1,100),(3,300),(4,400)",
    ] {
        tk.MustExec(statement, Vec::new());
    }
}

#[test]
fn predicate_pushdown_go_fixture_cases() {
    let suite = load_suite();
    for cascades in [false, true] {
        let mut tk = new_testkit(cascades);
        prepare_tables(&mut tk);
        run_fixture(&mut tk, &suite, cascades);
    }
}

#[test]
fn predicate_pushdown_issue_paths_execute() {
    for cascades in [false, true] {
        let mut tk = new_testkit(cascades);
        tk.MustExec(
            "create table crm_rd_150m (customer_first_date timestamp null default null)",
            Vec::new(),
        );
        tk.MustQuery("explain format = 'plan_tree' select count(*) from crm_rd_150m where (case when month(customer_first_date) <= 30 then 'new' else null end) is not null", Vec::new()).CheckContain("Table");
        tk.MustExec("create table t31202(a int primary key, b int)", Vec::new());
        tk.MustQuery(
            "explain format = 'plan_tree' select * from t31202 use index (primary)",
            Vec::new(),
        )
        .CheckContain("Table");
    }
}

#[test]
fn selection_predicates_push_through_projection_to_scan() {
    use astersql_planner_core::rule_predicate_push_down::PPDSolver;
    use astersql_planner_core::task::{PlanKind, PlanNode};

    let scan = PlanNode::new(PlanKind::TableScan);
    let projection = PlanNode::new(PlanKind::Projection).with_children(vec![scan]);
    let mut selection = PlanNode::new(PlanKind::Selection).with_children(vec![projection]);
    selection.conditions = vec![crate::support::expr("gt:a:1", Some(0))];
    let (result, changed) = PPDSolver.Optimize(selection);
    assert!(!changed, "Go's PPDSolver reports planChanged=false");
    assert_eq!(result.kind, PlanKind::Projection);
    assert_eq!(result.children[0].kind, PlanKind::TableScan);
    assert_eq!(result.children[0].conditions[0].name, "gt:a:1");
}
