// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use std::path::PathBuf;

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::{ConvertRowsToStrings, TestData};
use astersql_testkit::{Rows, TestKit};

fn new_test_kit(cascades: bool) -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut test_kit = TestKit::new(store);
    test_kit.MustExec("use test", Vec::new());
    test_kit.MustExec(
        &format!(
            "set @@tidb_enable_cascades_planner = {}",
            if cascades { "on" } else { "off" }
        ),
        Vec::new(),
    );
    test_kit
}

fn prepare_outer_to_semi_tables(test_kit: &mut TestKit) {
    for statement in [
        "drop table if exists A, B, t1, t2, t3",
        "CREATE TABLE A (id INT, val INT, nullable_val INT)",
        "CREATE TABLE B (id INT PRIMARY KEY, a_id INT, val INT, non_null_col INT NOT NULL, nullable_col INT)",
        "INSERT INTO A VALUES (1, 10, 100), (2, 20, NULL), (3, NULL, 300), (4, 40, 400), (NULL, 50, 500)",
        "INSERT INTO B VALUES (101, 1, 10, 1, 1), (102, 2, NULL, 2, NULL), (103, 5, 500, 5, 5), (104, NULL, 600, 6, 6)",
        "CREATE TABLE t1 (i INT NOT NULL)",
        "INSERT INTO t1 VALUES (0), (2), (3), (4)",
        "CREATE TABLE t2 (i INT NOT NULL)",
        "INSERT INTO t2 VALUES (0), (1), (3), (4)",
        "CREATE TABLE t3 (i INT NOT NULL)",
        "INSERT INTO t3 VALUES (0), (1), (2), (4)",
    ] {
        test_kit.MustExec(statement, Vec::new());
    }
}

/// Execute every Go golden case in both planner modes, including plan and result checks.
#[test]
fn outer_to_semi_go_golden_cases_execute_in_both_planners() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let suite = TestData::load_with_cascades(&directory, "outer_to_semi_join_suite", true)
        .expect("load Go outer-to-semi fixtures");

    for cascades in [false, true] {
        let mut test_kit = new_test_kit(cascades);
        prepare_outer_to_semi_tables(&mut test_kit);
        let mut mismatches = Vec::new();
        let (input, output) = suite
            .LoadTestCasesByName("TestOuterToSemiJoin", cascades)
            .expect("load TestOuterToSemiJoin cases");
        let input = input.as_array().expect("outer-to-semi input is an array");
        let output = output.as_array().expect("outer-to-semi output is an array");
        assert_eq!(input.len(), output.len(), "Go fixture case count");

        for (index, (sql, expected)) in input.iter().zip(output).enumerate() {
            let sql = sql
                .as_str()
                .unwrap_or_else(|| panic!("input case {index} is not SQL"));
            assert_eq!(expected["SQL"].as_str(), Some(sql), "SQL case {index}");
            let expected_plan = expected["Plan"]
                .as_array()
                .unwrap_or_else(|| panic!("plan case {index} is not an array"))
                .iter()
                .map(|row| row.as_str().expect("plan row is a string").to_owned())
                .collect::<Vec<_>>();
            let expected_result = expected["Result"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .map(|row| row.as_str().expect("result row is a string"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| {
                    assert!(expected["Result"].is_null(), "result case {index} shape");
                    Vec::new()
                });

            let actual_plan = ConvertRowsToStrings(
                &test_kit
                    .MustQuery(&format!("EXPLAIN FORMAT='plan_tree' {sql}"), Vec::new())
                    .Rows(),
            );
            let actual_result = test_kit.MustQuery(sql, Vec::new()).Rows();
            if actual_plan != expected_plan {
                mismatches.push(format!(
                    "case {index} plan: expected={expected_plan:?}, actual={actual_plan:?}"
                ));
            }
            if actual_result != Rows(&expected_result) {
                mismatches.push(format!(
                    "case {index} result: expected={:?}, actual={actual_result:?}",
                    Rows(&expected_result)
                ));
            }
        }

        for statement in [
            "drop table if exists t_outer, t_inner",
            "create table t_outer (id int not null)",
            "create table t_inner (id int not null, d varchar(64) not null)",
            "insert into t_outer values (1), (3)",
            "insert into t_inner values (2, 'alpha')",
        ] {
            test_kit.MustExec(statement, Vec::new());
        }
        test_kit
            .MustQuery(
                "select d from ((select t_inner.d from t_outer left join t_inner on t_outer.id = t_inner.id where t_inner.id is null) union all select 'z') y order by d",
                Vec::new(),
            )
            .Check(Rows(&["<nil>", "<nil>", "z"]));
        assert!(
            mismatches.is_empty(),
            "planner={} mismatches: {}",
            if cascades { "on" } else { "off" },
            mismatches.join(", ")
        );
    }
}

/// Go issue #58829: both hint paths must permit IndexHashJoin.
#[test]
fn semi_join_rewrite_hint_matches_go() {
    for cascades in [false, true] {
        let mut test_kit = new_test_kit(cascades);
        test_kit.MustExec(
            "create table t1 (id varchar(64) not null, key(id))",
            Vec::new(),
        );
        test_kit.MustExec("create table t2 (id bigint(20), k int)", Vec::new());
        test_kit.MustExec("insert into t1 values ('1'), ('2'), ('3')", Vec::new());
        test_kit.MustExec("insert into t2 values (1, 1), (2, 0)", Vec::new());
        assert!(test_kit.HasPlan(
            "delete from t1 where t1.id in (select /*+ semi_join_rewrite() */ /* issue:58829 */ cast(id as char) from t2 where k=1)",
            "IndexHashJoin",
        ));
        test_kit.MustExec(
            "delete from t1 where t1.id in (select /*+ semi_join_rewrite() */ cast(id as char) from t2 where k=1)",
            Vec::new(),
        );
        test_kit
            .MustQuery("select id from t1 order by id", Vec::new())
            .Check(Rows(&["2", "3"]));
        test_kit.MustExec("insert into t1 values ('1')", Vec::new());
        test_kit.MustExec(
            "set @@tidb_opt_enable_alternative_logical_plans=on",
            Vec::new(),
        );
        test_kit.MustExec("set @@tidb_opt_enable_semi_join_rewrite=off", Vec::new());
        assert!(test_kit.HasPlan(
            "delete from t1 where t1.id in (select /* issue:58829 */ cast(id as char) from t2 where k=1)",
            "IndexHashJoin",
        ));
    }
}

#[test]
fn semi_join_rewrite_builds_grouped_inner_join() {
    use astersql_planner_core::rule_join_reorder::JoinNode;
    use astersql_planner_core::rule_semi_join_rewrite::SemiJoinRewriter;
    use astersql_planner_core::task::JoinType;

    let semi = crate::support::join(
        3,
        JoinType::Semi,
        crate::support::leaf(1, "outer", vec![1], 10.0),
        crate::support::leaf(2, "inner", vec![2], 10.0),
        Some((1, 2)),
    );
    let (result, changed) = SemiJoinRewriter.Optimize(semi).unwrap();
    assert!(!changed, "Go SemiJoinRewriter reports planChanged=false");
    match result.node {
        JoinNode::Projection { child, .. } => match child.node {
            JoinNode::Join {
                join_type, right, ..
            } => {
                assert_eq!(join_type, JoinType::Inner);
                match right.node {
                    JoinNode::Aggregation { group_by, .. } => assert_eq!(group_by, vec![2]),
                    _ => panic!("inner side must be grouped"),
                }
            }
            _ => panic!("projection child must be inner join"),
        },
        _ => panic!("semi join must become projection"),
    }
}
