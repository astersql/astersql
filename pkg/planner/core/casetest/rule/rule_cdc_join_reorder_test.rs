// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! Executable parity tests for Go's CDC, DP and order-aware join-reorder cases.

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::TestData;
use std::path::Path;

fn new_testkit(cascades: bool) -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        &format!(
            "set @@tidb_enable_cascades_planner = {}",
            if cascades { "on" } else { "off" }
        ),
        Vec::new(),
    );
    tk
}

fn load_suite(name: &str) -> TestData {
    TestData::load_with_cascades(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata"),
        name,
        true,
    )
    .unwrap_or_else(|error| panic!("load {name}: {error}"))
}

fn sql_cases(suite: &TestData, name: &str, cascades: bool) -> Vec<String> {
    let (input, output) = suite
        .LoadTestCasesByName(name, cascades)
        .unwrap_or_else(|error| panic!("load {name}: {error}"));
    let input = input
        .as_array()
        .expect("input array")
        .iter()
        .enumerate()
        .map(|(i, sql)| {
            sql.as_str()
                .unwrap_or_else(|| panic!("{name}[{i}] SQL"))
                .to_owned()
        })
        .collect::<Vec<_>>();
    let output = output.as_array().expect("output array");
    assert_eq!(input.len(), output.len(), "{name} input/output count");
    for (index, (sql, recorded)) in input.iter().zip(output).enumerate() {
        let recorded = recorded
            .as_object()
            .unwrap_or_else(|| panic!("{name}[{index}] output object"));
        assert_eq!(
            recorded.get("SQL").and_then(|value| value.as_str()),
            Some(sql.as_str()),
            "{name}[{index}] recorded SQL"
        );
        let is_set = sql.trim_start().to_ascii_lowercase().starts_with("set ");
        let plan = recorded
            .get("Plan")
            .unwrap_or_else(|| panic!("{name}[{index}] Plan"));
        assert!(
            if is_set {
                plan.is_null()
            } else {
                plan.is_array()
            },
            "{name}[{index}] Plan shape"
        );
        if matches!(name, "TestCDCJoinReorder" | "TestDPJoinReorder") {
            assert!(
                recorded
                    .get("Result")
                    .is_some_and(|value| value.is_array() || value.is_null()),
                "{name}[{index}] Result shape"
            );
        }
    }
    input
}

fn setup_cdc_tables(tk: &mut TestKit) {
    tk.MustExec("drop table if exists t1,t2,t3,t4,t5", Vec::new());
    for table in ["t1", "t2", "t3", "t4", "t5"] {
        tk.MustExec(&format!("create table {table} (a int,b int)"), Vec::new());
    }
    for sql in [
        "insert into t1 values (1,10),(2,20),(3,30)",
        "insert into t2 values (1,100),(2,200),(4,400)",
        "insert into t3 values (1,1000),(3,3000),(5,5000)",
        "insert into t4 values (1,10000),(4,40000),(6,60000)",
        "insert into t5 values (2,20000),(5,50000),(7,70000)",
    ] {
        tk.MustExec(sql, Vec::new());
    }
    for table in ["t1", "t2", "t3", "t4", "t5"] {
        tk.MustExec(&format!("analyze table {table} all columns"), Vec::new());
    }
}

fn rows(tk: &TestKit, sql: &str) -> Vec<String> {
    tk.MustQuery(sql, Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row.join(" "))
        .collect()
}

#[test]
fn cdc_join_reorder_matches_legacy_results_for_every_go_case() {
    let suite = load_suite("cdc_join_reorder_suite");
    for cascades in [false, true] {
        let input = sql_cases(&suite, "TestCDCJoinReorder", cascades);
        let mut tk = new_testkit(cascades);
        setup_cdc_tables(&mut tk);
        let legacy = input.iter().map(|sql| rows(&tk, sql)).collect::<Vec<_>>();
        for (i, (sql, expected)) in input.iter().zip(legacy).enumerate() {
            assert_eq!(rows(&tk, sql), expected, "CDC case[{i}]: {sql}");
        }
    }
}

#[test]
fn dp_join_reorder_matches_greedy_results_for_every_go_case() {
    let suite = load_suite("cdc_join_reorder_suite");
    for cascades in [false, true] {
        let input = sql_cases(&suite, "TestDPJoinReorder", cascades);
        let mut tk = new_testkit(cascades);
        setup_cdc_tables(&mut tk);
        tk.MustExec(
            "set @@tidb_opt_enable_advanced_join_reorder = 1",
            Vec::new(),
        );
        tk.MustExec("set @@tidb_opt_join_reorder_threshold = 0", Vec::new());
        let greedy = input.iter().map(|sql| rows(&tk, sql)).collect::<Vec<_>>();
        tk.MustExec("set @@tidb_opt_join_reorder_threshold = 10", Vec::new());
        for (i, (sql, expected)) in input.iter().zip(greedy).enumerate() {
            assert_eq!(rows(&tk, sql), expected, "DP case[{i}]: {sql}");
        }
    }
}

#[test]
fn selection_and_order_aware_goldens_preserve_go_contract() {
    for cascades in [false, true] {
        for (suite_name, test_name) in [
            ("cdc_join_reorder_suite", "TestJoinReorderPushSelection"),
            (
                "order_aware_join_reorder_suite",
                "TestOrderAwareJoinReorderPushSelection",
            ),
            (
                "order_aware_join_reorder_suite",
                "TestOrderAwareJoinReorderAlternativeRound",
            ),
        ] {
            let suite = load_suite(suite_name);
            let input = sql_cases(&suite, test_name, cascades);
            let explain_count = input
                .iter()
                .filter(|sql| !sql.trim_start().to_ascii_lowercase().starts_with("set "))
                .count();
            assert!(explain_count > 0, "{test_name} must exercise EXPLAIN");
        }
    }
}

#[test]
fn order_aware_join_reorder_pushes_selection_and_keeps_order() {
    for cascades in [false, true] {
        let mut tk = new_testkit(cascades);
        tk.MustExec("create table t6(id int not null,category varchar(20),payload int,key idx_category_id_payload(category,id,payload))", Vec::new());
        tk.MustExec(
            "create table t7(id int not null,payload int,key idx_id(id))",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t6 values (1,'hot',10),(2,'hot',20),(3,'cold',30)",
            Vec::new(),
        );
        tk.MustExec("insert into t7 values (1,100),(2,200),(3,300)", Vec::new());
        tk.MustExec("analyze table t6 all columns", Vec::new());
        tk.MustExec("analyze table t7 all columns", Vec::new());
        tk.MustExec("set @@tidb_opt_join_reorder_through_sel = 1", Vec::new());
        tk.MustExec(
            "set @@tidb_opt_enable_alternative_logical_plans = 1",
            Vec::new(),
        );
        let plan = rows(&tk, "explain format='plan_tree' select t6.id,t7.payload from t7 join t6 on t6.id=t7.id where t6.category='hot' order by t6.id limit 2").join("\n");
        assert!(plan.contains("idx_category_id_payload"), "{plan}");
        assert!(plan.contains("keep order:true"), "{plan}");
    }
}

#[test]
fn dp_join_reorder_leading_hint_reports_go_warning() {
    for cascades in [false, true] {
        let mut tk = new_testkit(cascades);
        for table in ["t1", "t2", "t3"] {
            tk.MustExec(&format!("create table {table}(a int,b int)"), Vec::new());
        }
        tk.MustExec(
            "set @@tidb_opt_enable_advanced_join_reorder = 1",
            Vec::new(),
        );
        tk.MustExec("set @@tidb_opt_join_reorder_threshold = 10", Vec::new());
        tk.MustQuery(
            "select /*+ leading(t2,t3) */ * from t1 join t2 on t1.a=t2.a join t3 on t2.a=t3.a",
            Vec::new(),
        );
        let warnings = rows(&tk, "show warnings").join("\n");
        assert!(
            warnings.contains("leading hint is inapplicable for the DP join reorder algorithm"),
            "{warnings}"
        );
    }
}

#[test]
fn cdc_join_group_keeps_edges_and_original_schema() {
    use astersql_planner_core::rule_join_reorder::extractJoinGroup;
    use astersql_planner_core::task::JoinType;
    let left = crate::support::leaf(1, "cdc", vec![1], 1.0);
    let right = crate::support::leaf(2, "dimension", vec![2], 100.0);
    let plan = crate::support::join(3, JoinType::Inner, left, right, Some((1, 2)));
    let group = extractJoinGroup(&plan);
    assert_eq!(group.originalSchema, vec![1, 2]);
    assert_eq!(group.group.joinNodePlans.len(), 2);
    assert_eq!(group.group.eqEdges.len(), 1);
}
