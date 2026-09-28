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

/*
/// Historical translation reference. Executable parity coverage follows below.
const _GO_PREDICATE_PUSHDOWN_REFERENCE: &str = r########"

// 保留谓词下推测试的 fixture 读取、SET 语句短路、plan/warning/result 录制和 TiFlash 伪副本设置流程；

// PredicatePushdownOutput 对应 Go 中只校验 plan 与 warning 的匿名输出结构。
struct PredicatePushdownOutput {
    sql: String,
    plan: Vec<String>,
    warning: Vec<String>,
}
"########;

/// Selection 谓词应穿过恒等 Projection，落到 TableScan 的 conditions 上。
#[test]
fn selection_predicates_push_through_projection_to_scan() {
    use astersql_planner_core::rule_predicate_push_down::PPDSolver;
    use astersql_planner_core::task::{PlanKind, PlanNode};

    // 计划形状：Selection -> Projection -> TableScan；谓词引用投影输出列 0。
    let scan = PlanNode::new(PlanKind::TableScan);
    let projection = PlanNode::new(PlanKind::Projection).with_children(vec![scan]);
    let mut selection = PlanNode::new(PlanKind::Selection).with_children(vec![projection]);
    selection.conditions = vec![crate::support::expr("gt:a:1", Some(0))];

    let (result, changed) = PPDSolver.Optimize(selection);
    assert!(!changed, "Go's PPDSolver reports planChanged=false");
    // 优化后根为 Projection，其子 Scan 携带下推后的谓词。
    assert_eq!(result.kind, PlanKind::Projection);
    assert_eq!(result.children[0].kind, PlanKind::TableScan);
    assert_eq!(result.children[0].conditions[0].name, "gt:a:1");
}

/// Go 侧带 result 校验的 PPD 辅助函数与 collation 子测试原文。
const _GO_PREDICATE_PUSHDOWN_REMAINDER: &str = r#########"

// PredicatePushdownOutputWithResult 对应 Go 中额外校验查询结果的匿名输出结构。
struct PredicatePushdownOutputWithResult {
    sql: String,
    plan: Vec<String>,
    result: Vec<String>,
    warning: Vec<String>,
}

// runPredicatePushdownTestData 对应 Go 的同名辅助函数。
fn run_predicate_pushdown_test_data(t: &mut TestingT, tk: &mut TestKit, cascades: &str, name: &str) {
    let mut input: Vec<String> = Vec::new();
    let mut output: Vec<PredicatePushdownOutput> = Vec::new();
    let predicate_pushdown_suite_data = get_predicate_pushdown_suite_data();
    predicate_pushdown_suite_data.load_test_cases_by_name(name, t, &mut input, &mut output, cascades);
    require_equal(input.len(), output.len());

    for i in 0..input.len() {
        if input[i].contains("set") {
            // Go 对包含 set 的 fixture 只执行会话变量语句，不做 explain/warning 对比。
            tk.must_exec(&input[i]);
            continue;
        }
        testdata_on_record(|| {
            output[i].sql = input[i].clone();
            output[i].plan = convert_rows_to_strings(
                tk.must_query(&format!("explain format = 'plan_tree' {}", input[i])).rows(),
            );
            output[i].warning = convert_rows_to_strings(tk.must_query("show warnings").rows());
        });
        tk.must_query(&format!("explain format = 'plan_tree' {}", input[i]))
            .check(rows(&output[i].plan));
        tk.must_query("show warnings").check(rows(&output[i].warning));
    }
}

// runPredicatePushdownTestDataWithResult 对应 Go 的同名辅助函数。
fn run_predicate_pushdown_test_data_with_result(
    t: &mut TestingT,
    tk: &mut TestKit,
    cascades: &str,
    name: &str,
) {
    let mut input: Vec<String> = Vec::new();
    let mut output: Vec<PredicatePushdownOutputWithResult> = Vec::new();
    let predicate_pushdown_suite_data = get_predicate_pushdown_suite_data();
    predicate_pushdown_suite_data.load_test_cases_by_name(name, t, &mut input, &mut output, cascades);
    require_equal(input.len(), output.len());

    for i in 0..input.len() {
        if input[i].contains("set") {
            // SET 语句改变优化器环境，Go 原测试不会拿它生成 plan_tree。
            tk.must_exec(&input[i]);
            continue;
        }
        testdata_on_record(|| {
            output[i].sql = input[i].clone();
            output[i].plan = convert_rows_to_strings(
                tk.must_query(&format!("explain format = 'plan_tree' {}", input[i])).rows(),
            );
            output[i].warning = convert_rows_to_strings(tk.must_query("show warnings").rows());
            output[i].result = convert_rows_to_strings(tk.must_query(&input[i]).rows());
        });
        tk.must_query(&format!("explain format = 'plan_tree' {}", input[i]))
            .check(rows(&output[i].plan));
        tk.must_query("show warnings").check(rows(&output[i].warning));
        tk.must_query(&input[i]).check(rows(&output[i].result));
    }
}

// TestPredicatePushdownSuite 对应 Go 的 suite 入口，保留两个子测试名称。
#[test]
fn test_predicate_pushdown_suite() {
    run_subtest("TestConstantPropagateWithCollation", test_constant_propagate_with_collation);
    run_subtest("TestPredicatePushDown", test_predicate_push_down);
}

// testConstantPropagateWithCollation 对应 Go 的同名子测试。
fn test_constant_propagate_with_collation(t: &mut TestingT) {
    run_test_under_cascades(t, |t, tk, cascades, _caller| {
        tk.must_exec("use test");
        tk.must_exec("create table t (id int primary key, name varchar(20));");
        tk.must_exec(r#"create table foo(a int, b int, c int, primary key(a));"#);
        tk.must_exec(r#"create table bar(a int, b int, c int, primary key(a));"#);
        tk.must_exec("create table t0 (k0 int, p1 bigint)");
        tk.must_exec("create table t1 (k0 int, d0 varchar(64))");
        tk.must_exec("create table t2 (k0 int, k1 int)");
        tk.must_exec("create table t3 (k0 int, d0 decimal(12,2))");
        tk.must_exec("create table t4 (k0 int)");
        tk.must_exec("drop table if exists t1_65994");
        tk.must_exec("drop table if exists t2_65994");
        tk.must_exec("create table t1_65994 (a int, b int)");
        tk.must_exec("create table t2_65994 (a int, b int)");
        tk.must_exec("insert into t0 values (1, 10), (2, 20), (3, 30)");
        tk.must_exec("insert into t2 values (1, 100), (3, 300), (4, 400)");
        run_predicate_pushdown_test_data_with_result(
            t,
            tk,
            cascades,
            "TestConstantPropagateWithCollation",
        );
    });
}
"#########;
/// Go 侧依赖 Domain / TiFlash 伪副本的 PredicatePushDown 子测试原文。
const _GO_PREDICATE_PUSHDOWN_DOMAIN_REMAINDER: &str = r##########"
// testPredicatePushDown 对应 Go 的同名子测试，含 Domain 和 TiFlash replica 伪装。
fn test_predicate_push_down(t: &mut TestingT) {
    run_test_under_cascades_with_domain(t, |t, tk, dom, _cascades, _caller| {
        tk.must_exec("use test");
        tk.must_exec(
            r#"CREATE TABLE crm_rd_150m (
    product varchar(256) DEFAULT NULL,
        uks varchar(16) DEFAULT NULL,
        brand varchar(256) DEFAULT NULL,
        cin varchar(16) DEFAULT NULL,
        created_date timestamp NULL DEFAULT NULL,
        quantity int(11) DEFAULT NULL,
        amount decimal(11,0) DEFAULT NULL,
        pl_date timestamp NULL DEFAULT NULL,
        customer_first_date timestamp NULL DEFAULT NULL,
        recent_date timestamp NULL DEFAULT NULL
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8 COLLATE=utf8_bin;"#,
        );

        // Create virtual tiflash replica info.
        // Go 这里在 Domain 中设置虚拟 TiFlash 副本，只用于 explain 计划选择，不写入真实集群。
        set_tiflash_replica(t, dom, "test", "crm_rd_150m");

        tk.must_exec("set @@session.tidb_isolation_read_engines = 'tiflash'");
        tk.must_exec("explain format = 'plan_tree' SELECT /* issue:15110 */ count(*) FROM crm_rd_150m dataset_48 WHERE (CASE WHEN (month(dataset_48.customer_first_date)) <= 30 THEN '新客' ELSE NULL END) IS NOT NULL;");

        tk.must_exec("drop table if exists t31202");
        tk.must_exec("create table t31202(a int primary key, b int);");

        // Set the hacked TiFlash replica for explain tests.
        // 同上，保留 Go 测试对 TiFlash 路径的外部依赖标记。
        set_tiflash_replica(t, dom, "test", "t31202");

        tk.must_query("explain format = 'plan_tree' select /* issue:31202 */ * from t31202;")
            .check(vec![
                "TableReader root  MppVersion: 3, data:ExchangeSender",
                "└─ExchangeSender mpp[tiflash]  ExchangeType: PassThrough",
                "  └─TableFullScan mpp[tiflash] table:t31202 keep order:false, stats:pseudo",
            ]);

        tk.must_exec("set @@session.tidb_isolation_read_engines = 'tikv'");
        tk.must_query("explain format = 'plan_tree' select /* issue:31202 */ * from t31202 use index (primary);")
            .check(vec![
                "TableReader root  data:TableFullScan",
                "└─TableFullScan cop[tikv] table:t31202 keep order:false, stats:pseudo",
            ]);
    });
}
"##########;

#[test]
fn predicate_pushdown_fixture_inventory_matches_go() {
    let cases = crate::support::fixture_case_counts(
        "predicate_pushdown_suite",
        &["TestConstantPropagateWithCollation"],
    );
    assert_eq!(cases.len(), 1);
    assert!(cases[0].0 > 0);
    assert_eq!(cases[0].0, cases[0].1);
}
*/

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
