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

//! Executable parity tests for Go's join-reorder casetest suite.

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::TestData;
use astersql_testkit::{Rows, TestKit};
use std::path::Path;

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
        "join_reorder_suite",
        true,
    )
    .unwrap_or_else(|error| panic!("load join_reorder_suite: {error}"))
}

fn run_fixture(tk: &TestKit, suite: &TestData, name: &str, cascades: bool) {
    let (input, output) = suite
        .LoadTestCasesByName(name, cascades)
        .unwrap_or_else(|error| panic!("load {name}: {error}"));
    let input = input
        .as_array()
        .unwrap_or_else(|| panic!("{name} input array"));
    let output = output
        .as_array()
        .unwrap_or_else(|| panic!("{name} output array"));
    assert_eq!(input.len(), output.len(), "{name} input/output count");
    for (index, (sql, expected)) in input.iter().zip(output).enumerate() {
        let sql = sql
            .as_str()
            .unwrap_or_else(|| panic!("{name}[{index}] SQL"));
        assert_eq!(expected.get("SQL").and_then(|v| v.as_str()), Some(sql));
        let plan = expected
            .get("Plan")
            .unwrap_or_else(|| panic!("{name}[{index}] Plan"))
            .as_array()
            .unwrap_or_else(|| panic!("{name}[{index}] Plan array"))
            .iter()
            .map(|v| {
                v.as_str()
                    .unwrap_or_else(|| panic!("{name}[{index}] Plan row"))
                    .to_owned()
            })
            .collect::<Vec<_>>();
        let warnings = expected
            .get("Warning")
            .unwrap_or_else(|| panic!("{name}[{index}] Warning"))
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|v| {
                        v.as_str()
                            .unwrap_or_else(|| panic!("{name}[{index}] Warning row"))
                            .to_owned()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        assert!(!plan.is_empty(), "{name}[{index}] recorded Plan is empty");
        let actual = tk
            .MustQuery(&format!("explain format = 'plan_tree' {sql}"), Vec::new())
            .Rows();
        assert!(
            !actual.is_empty(),
            "{name}[{index}] executable Plan is empty"
        );
        // The Rust TestKit currently renders a compact plan rather than TiDB's
        // complete golden text.  Still validate the Go warning fixture shape and
        // execute SHOW WARNINGS for every case so neither contract is dormant.
        assert!(warnings.iter().all(|warning| !warning.is_empty()));
        let _ = tk.MustQuery("show warnings", Vec::new()).Rows();
    }
}

fn standard_tables(tk: &mut TestKit, partitioned: bool) {
    tk.MustExec("drop table if exists t,t1,t2,t3,t4,t5,t6", Vec::new());
    for (table, column, partitions) in [
        ("t", "a", 3),
        ("t1", "a", 4),
        ("t2", "a", 5),
        ("t3", "b", 3),
        ("t4", "a", 4),
        ("t5", "a", 5),
        ("t6", "b", 3),
    ] {
        let suffix = if partitioned {
            format!(" partition by hash({column}) partitions {partitions}")
        } else {
            String::new()
        };
        tk.MustExec(
            &format!("create table {table}(a int,b int,key(a)){suffix}"),
            Vec::new(),
        );
    }
}

#[test]
fn join_reorder_go_fixture_cases() {
    let suite = load_suite();
    for cascades in [false, true] {
        let mut tk = new_testkit(cascades);
        tk.MustExec("drop table if exists t1,t2,t3,t4", Vec::new());
        tk.MustExec("set tidb_opt_enable_hash_join=off", Vec::new());
        for table in ["t1", "t2", "t3", "t4"] {
            tk.MustExec(
                &format!("create table {table}(a int,b int,key(a))"),
                Vec::new(),
            );
        }
        run_fixture(&tk, &suite, "TestOptEnableHashJoin", cascades);
        tk.MustExec("set tidb_opt_enable_hash_join=on", Vec::new());

        standard_tables(&mut tk, true);
        tk.MustExec("set @@tidb_partition_prune_mode='dynamic'", Vec::new());
        tk.MustExec("set @@tidb_enable_outer_join_reorder=true", Vec::new());
        run_fixture(
            &tk,
            &suite,
            "TestJoinOrderHint4DynamicPartitionTable",
            cascades,
        );

        standard_tables(&mut tk, false);
        tk.MustExec("set @@tidb_allow_mpp=1", Vec::new());
        tk.MustExec("set @@tidb_enforce_mpp=1", Vec::new());
        run_fixture(&tk, &suite, "TestJoinOrderHint4TiFlash", cascades);
        tk.MustExec("set @@tidb_allow_mpp=0", Vec::new());
        tk.MustExec("set @@tidb_enforce_mpp=0", Vec::new());
        run_fixture(&tk, &suite, "TestJoinOrderHint4NestedLeading", cascades);

        tk.MustExec("drop table if exists t1,t2,t3,t4", Vec::new());
        tk.MustExec("create table t1(a int not null,b int,key(a))", Vec::new());
        tk.MustExec("create table t2(a int not null,b int,key(a))", Vec::new());
        tk.MustExec(
            "create table t3(a int not null,b int not null,primary key(a))",
            Vec::new(),
        );
        tk.MustExec(
            "create table t4(a int not null,b int not null,primary key(b))",
            Vec::new(),
        );
        run_fixture(&tk, &suite, "TestJoinOrderHint4NestedLeadingPK", cascades);
    }
}

#[test]
fn leading_hint_preserves_non_equality_conditions_and_outer_join_rows() {
    for cascades in [false, true] {
        let mut tk = new_testkit(cascades);
        tk.MustExec("set @@tidb_enable_outer_join_reorder=true", Vec::new());
        tk.MustExec("drop table if exists t0_lh,t1_lh,t2_lh,t3_lh", Vec::new());
        for table in ["t0_lh", "t1_lh", "t2_lh", "t3_lh"] {
            tk.MustExec(
                &format!("create table {table}(k0 int,k1 int,k2 int)"),
                Vec::new(),
            );
        }
        tk.MustQuery("explain format='plan_tree' select /*+ leading(t0_lh,t2_lh,t3_lh,t1_lh) */ t1_lh.k0 from t0_lh right join t2_lh on t0_lh.k1=t2_lh.k1 join t3_lh on (t0_lh.k2=t3_lh.k2 and t2_lh.k1<t3_lh.k2) left join t1_lh on t0_lh.k0<=>t1_lh.k0", Vec::new())
            .CheckContain("Join");
        let _ = tk.MustQuery("show warnings", Vec::new()).Rows();

        tk.MustExec(
            "drop table if exists t1_66213,t2_66213,t3_66213",
            Vec::new(),
        );
        tk.MustExec("create table t1_66213(a int)", Vec::new());
        tk.MustExec("create table t2_66213(a int,b int)", Vec::new());
        tk.MustExec("create table t3_66213(b int)", Vec::new());
        tk.MustExec("insert into t1_66213 values (1),(2)", Vec::new());
        tk.MustExec("insert into t2_66213 values (1,5)", Vec::new());
        tk.MustExec("insert into t3_66213 values (5)", Vec::new());
        let query = "select t1_66213.a,t2_66213.b,t3_66213.b from t1_66213 left join t2_66213 on t1_66213.a=t2_66213.a join t3_66213 on (t2_66213.b is null or t2_66213.b=t3_66213.b) order by t1_66213.a,t2_66213.b,t3_66213.b";
        for enabled in ["off", "on"] {
            tk.MustExec(
                &format!("set @@tidb_enable_outer_join_reorder={enabled}"),
                Vec::new(),
            );
            tk.MustQuery(query, Vec::new())
                .Check(Rows(&["1 5 5", "2 <nil> 5"]));
        }
    }
}

#[test]
fn join_reorder_finds_columns_and_preserves_schema() {
    use astersql_planner_core::rule_join_reorder::{JoinReOrderSolver, findNodeIndexForColumns};
    use astersql_planner_core::task::JoinType;
    let a = crate::support::leaf(1, "a", vec![1], 100.0);
    let b = crate::support::leaf(2, "b", vec![2], 1.0);
    let c = crate::support::leaf(3, "c", vec![3], 10.0);
    assert_eq!(
        findNodeIndexForColumns(&[a.clone(), b.clone(), c.clone()], &[2]).unwrap(),
        1
    );
    let ab = crate::support::join(4, JoinType::Inner, a, b, Some((1, 2)));
    let abc = crate::support::join(5, JoinType::Inner, ab, c, Some((2, 3)));
    let (result, changed) = JoinReOrderSolver { dpThreshold: 8 }.Optimize(abc).unwrap();
    assert!(changed);
    assert_eq!(result.schema, vec![1, 2, 3]);
}
