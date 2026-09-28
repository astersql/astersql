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

//! Window 下推 casetest。
//!
//! Go 版本通过 `window_push_down_suite` 驱动 MPP/TiFlash plan golden；这里保留同一
//! fixture 的完整清单，并用生产 `DeriveTopNFromWindow` 验证该 suite 所覆盖的规则
//! 不变量：紧邻 Window 的 `row_number() <= N` 才能派生 TopN，分区键和 MPP 标志不变。

use astersql_planner_core::rule_derive_topn_from_window::DeriveTopNFromWindow;
use astersql_planner_core::task::{Expression, PlanFlags, PlanKind, PlanNode};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::ConvertRowsToStrings;

const GO_WINDOW_PUSH_DOWN_TESTS: [&str; 3] = [
    "TestWindowFunctionDescCanPushDown",
    "TestWindowPushDownPlans",
    "TestWindowPlanWithOtherOperators",
];

fn partition_col(index: usize) -> Expression {
    Expression {
        name: format!("col{index}"),
        column: Some(index),
        ..Default::default()
    }
}

fn row_number_bound(bound: u64) -> Expression {
    Expression {
        name: format!("row_number_le:{bound}"),
        ..Default::default()
    }
}

fn window_with_partition(child: PlanNode, partition: &[usize], mpp_enforced: bool) -> PlanNode {
    let mut window = PlanNode::new(PlanKind::Window).with_children(vec![child]);
    window.by_items = partition.iter().copied().map(partition_col).collect();
    window.flags = PlanFlags {
        mpp_enforced,
        ..Default::default()
    };
    window
}

/// Go `TestWindowFunctionDescCanPushDown`/`TestWindowPushDownPlans`/`TestWindowPlanWithOtherOperators`
/// 的全部 SQL case 必须仍在输入、标准输出和 Cascades 输出中各有对应项。
#[test]
fn window_push_down_suite_inventory_matches_go() {
    super::main_test::assert_window_suite_inventory();
}

fn setup_employee_case() -> (std::sync::Arc<astersql_domain::Domain>, TestKit) {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists employee", Vec::new());
    tk.MustExec(
        "create table employee (empid int, deptid int, salary decimal(10,2))",
        Vec::new(),
    );
    // Go `RunTestUnderCascadesWithDomain` supplies these defaults through the
    // test session; make the same MPP/TiFlash inputs explicit for Rust TestKit.
    tk.MustExec("set @@tidb_enforce_mpp=1", Vec::new());
    tk.MustExec("set @@session.tidb_allow_mpp=1", Vec::new());
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=1", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines = 'tiflash'",
        Vec::new(),
    );
    domain
        .set_tiflash_replica_for_test("test", "employee", 1, true)
        .expect("set employee TiFlash replica");
    (domain, tk)
}

fn planner_warnings(tk: &TestKit) -> Vec<String> {
    tk.MustQuery("show warnings", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| {
            assert_eq!(row.len(), 3, "SHOW WARNINGS row shape");
            if row[2].starts_with('[') {
                row[2].clone()
            } else {
                format!("[planner:{}]{}", row[1], row[2])
            }
        })
        .collect()
}

/// 对齐 Go `testWithData`：SET/UPDATE 走执行路径，其余 SQL 的 plan 与 warning
/// 都逐项对照同一 standard/Cascades golden，不能只校验 fixture 数量。
fn run_window_push_down_suite(test_name: &str, cascades: bool) {
    astersql_testkit_testsetup::SetupForCommonTest();
    let (_domain, mut tk) = setup_employee_case();
    tk.MustExec(
        &format!(
            "set @@session.tidb_enable_cascades_planner = {}",
            u8::from(cascades)
        ),
        Vec::new(),
    );

    let suite = super::main_test::get_window_push_down_suite_data();
    let (input, output) = suite
        .LoadTestCasesByName(test_name, cascades)
        .unwrap_or_else(|error| panic!("load {test_name} cascades={cascades}: {error}"));
    let input = input
        .as_array()
        .unwrap_or_else(|| panic!("{test_name} input must be an array"));
    let output = output
        .as_array()
        .unwrap_or_else(|| panic!("{test_name} output must be an array"));
    assert_eq!(output.len(), input.len(), "{test_name} case count");
    let mut mismatches = Vec::new();

    for (case_index, (sql, recorded)) in input.iter().zip(output).enumerate() {
        let sql = sql
            .as_str()
            .unwrap_or_else(|| panic!("{test_name} case {case_index} input is not SQL"));
        assert_eq!(
            recorded.get("SQL").and_then(|value| value.as_str()),
            Some(sql),
            "{test_name} case {case_index} recorded SQL"
        );

        let fixture_strings = |field: &str| {
            let value = recorded
                .get(field)
                .unwrap_or_else(|| panic!("fixture case is missing {field}"));
            if value.is_null() {
                return Vec::new();
            }
            value
                .as_array()
                .unwrap_or_else(|| panic!("fixture field {field} must be an array or null"))
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("fixture field {field} contains a non-string value")
                        })
                        .to_owned()
                })
                .collect::<Vec<_>>()
        };

        if sql.starts_with("set") || sql.starts_with("UPDATE") {
            tk.MustExec(sql, Vec::new());
            assert!(
                recorded.get("Plan").is_some_and(|value| value.is_null()),
                "{test_name} case {case_index} command Plan"
            );
            assert!(
                recorded.get("Warn").is_some_and(|value| value.is_null()),
                "{test_name} case {case_index} command Warn"
            );
            continue;
        }

        let actual_plan = ConvertRowsToStrings(&tk.MustQuery(sql, Vec::new()).Rows());
        let expected_plan = fixture_strings("Plan");
        if actual_plan != expected_plan {
            mismatches.push(format!(
                "case {case_index} plan sql={sql:?}\nactual={actual_plan:#?}\nexpected={expected_plan:#?}"
            ));
        }
        let actual_warnings = planner_warnings(&tk);
        let expected_warnings = fixture_strings("Warn");
        if actual_warnings != expected_warnings {
            mismatches.push(format!(
                "case {case_index} warnings sql={sql:?}\nactual={actual_warnings:#?}\nexpected={expected_warnings:#?}"
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{test_name} cascades={cascades} mismatches:\n{}",
        mismatches.join("\n\n")
    );
}

#[test]
fn window_go_golden_replay_matches_standard_and_cascades_planners() {
    for cascades in [false, true] {
        for test_name in GO_WINDOW_PUSH_DOWN_TESTS {
            run_window_push_down_suite(test_name, cascades);
        }
    }
}

/// Go suite 的 SQL 前置环境必须通过真实 TestKit 接线；窗口结果用真实输入输出回归。
#[test]
fn window_testkit_wiring_executes_real_window_result() {
    astersql_testkit_testsetup::SetupForCommonTest();
    let (_domain, mut tk) = setup_employee_case();
    tk.MustExec(
        "insert into employee values (1, 10, 100.00), (2, 10, 200.00)",
        Vec::new(),
    );
    let rows = ConvertRowsToStrings(
        &tk.MustQuery(
            "select row_number() over (partition by deptid order by empid) from employee",
            Vec::new(),
        )
        .Rows(),
    );
    assert_eq!(rows, vec!["1".to_owned(), "2".to_owned()]);

    let suite = super::main_test::get_window_push_down_suite_data();
    let (input, _) = suite
        .LoadTestCasesByName("TestWindowFunctionDescCanPushDown", false)
        .expect("load wired window suite input");
    assert_eq!(input.as_array().expect("window suite input").len(), 7);
}

/// Go `TestWindowPlanWithOtherOperators` issue 34765：outer join 后的窗口 schema
/// 必须能完整规划；即使 Rust 尚无同名 failpoint，也要执行真实回归 SQL，而不是以
/// 手工 `PlanNode` 代替该资源/错误路径。
#[test]
fn window_issue_34765_outer_join_schema_executes_real_explain() {
    astersql_testkit_testsetup::SetupForCommonTest();
    let (domain, mut tk) = setup_employee_case();
    tk.MustExec("drop table if exists t1, t2", Vec::new());
    tk.MustExec(
        "create table t1(c1 varchar(32), c2 datetime, c3 bigint, c4 varchar(64))",
        Vec::new(),
    );
    tk.MustExec("create table t2(b2 varchar(64))", Vec::new());
    tk.MustExec("set tidb_enforce_mpp=1", Vec::new());
    domain
        .set_tiflash_replica_for_test("test", "t1", 1, true)
        .expect("set t1 TiFlash replica");
    domain
        .set_tiflash_replica_for_test("test", "t2", 1, true)
        .expect("set t2 TiFlash replica");

    for cascades in [false, true] {
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner = {}",
                u8::from(cascades)
            ),
            Vec::new(),
        );
        let plan = tk.MustQuery(
            "explain format = 'plan_tree' select /* issue:34765 */ count(*) from \
             (select row_number() over (partition by c1 order by c2) num from \
             (select * from t1 left join t2 on t1.c4 = t2.b2) tem2) tx where num = 1",
            Vec::new(),
        );
        assert!(
            !plan.is_empty(),
            "issue 34765 explain must return a plan with cascades={cascades}"
        );
    }
}

/// Go suite 的 partition-by 与 MPP window 场景：row_number 上界下推后窗口元数据不变。
#[test]
fn window_row_number_bound_derives_topn_and_preserves_partition_by() {
    let scan = PlanNode::new(PlanKind::TableScan);
    let window = window_with_partition(scan, &[0, 1], true);
    let mut selection = PlanNode::new(PlanKind::Selection).with_children(vec![window]);
    selection.conditions = vec![row_number_bound(1)];

    let (result, changed) = DeriveTopNFromWindow.Optimize(selection);
    assert!(!changed, "Go DeriveTopN reports planChanged=false");

    let window_after = &result.children[0];
    assert_eq!(window_after.kind, PlanKind::Window);
    assert!(window_after.flags.mpp_enforced);
    assert_eq!(
        window_after
            .by_items
            .iter()
            .map(|expression| expression.column)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(1)]
    );

    let topn = &window_after.children[0];
    assert_eq!(topn.kind, PlanKind::TopN);
    assert_eq!(topn.count, 1);
    assert_eq!(
        topn.by_items
            .iter()
            .map(|expression| expression.column)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(1)]
    );
    assert_eq!(topn.children[0].kind, PlanKind::TableScan);
}

/// Go `TestWindowPlanWithOtherOperators` 的多窗口形态不能被错误地扁平化。
#[test]
fn window_pushdown_keeps_nested_window_plan() {
    let scan = PlanNode::new(PlanKind::TableScan);
    let inner = window_with_partition(scan, &[0], false);
    let outer = window_with_partition(inner, &[1], true);
    let mut selection = PlanNode::new(PlanKind::Selection).with_children(vec![outer]);
    selection.conditions = vec![row_number_bound(3)];

    let (result, changed) = DeriveTopNFromWindow.Optimize(selection);
    assert!(!changed, "Go DeriveTopN reports planChanged=false");
    let outer_after = &result.children[0];
    assert_eq!(outer_after.kind, PlanKind::Window);
    assert!(outer_after.flags.mpp_enforced);
    assert_eq!(outer_after.children[0].kind, PlanKind::TopN);
    assert_eq!(outer_after.children[0].children[0].kind, PlanKind::Window);
    assert_eq!(
        outer_after.children[0].children[0].by_items[0].column,
        Some(0)
    );
}

/// 普通过滤（包括 Go issue 34765 的 `num = 1` 形态）不是可派生的 row_number 上界。
#[test]
fn non_row_number_window_predicate_is_left_untouched() {
    let scan = PlanNode::new(PlanKind::TableScan);
    let window = window_with_partition(scan, &[0], true);
    let mut selection = PlanNode::new(PlanKind::Selection).with_children(vec![window]);
    selection.conditions = vec![Expression {
        name: "eq".into(),
        column: Some(0),
        ..Default::default()
    }];

    let (result, changed) = DeriveTopNFromWindow.Optimize(selection);
    assert!(!changed);
    assert_eq!(result.kind, PlanKind::Selection);
    assert_eq!(result.children[0].kind, PlanKind::Window);
    assert_eq!(result.children[0].children[0].kind, PlanKind::TableScan);
}
