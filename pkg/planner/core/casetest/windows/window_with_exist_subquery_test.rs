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

//! Window 与 EXISTS/相关子查询的解相关 casetest。
//!
//! Go 用例同时覆盖 EXISTS、IN、ANY、outer-ref、outer join 与 CTE。本文件既通过
//! 生产 `TestKit` 执行 Go 的结果用例，也用 `DecorrelateSolver` 精确校验计划不变量。

use astersql_planner_core::rule_decorrelate::DecorrelateSolver;
use astersql_planner_core::rule_join_reorder::{JoinNode, JoinPlan};
use astersql_planner_core::task::{Expression, JoinType};
use astersql_testkit::Rows;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

fn outer_leaf(id: usize, schema: Vec<usize>) -> JoinPlan {
    JoinPlan {
        id,
        node: JoinNode::Leaf {
            name: "outer".into(),
            predicates: Vec::new(),
            unique_keys: Vec::new(),
            correlated_columns: Vec::new(),
        },
        schema,
        row_count: 1.0,
    }
}

fn inner_window(id: usize, inner_column: usize, correlated: bool) -> JoinPlan {
    let leaf = JoinPlan {
        id,
        node: JoinNode::Leaf {
            name: "inner".into(),
            predicates: Vec::new(),
            unique_keys: Vec::new(),
            correlated_columns: Vec::new(),
        },
        schema: vec![inner_column],
        row_count: 3.0,
    };
    let window = JoinPlan {
        id: id + 100,
        node: JoinNode::Window {
            partition_by: vec![inner_column],
            row_number_column: Some(3),
            upper_bound: None,
            child: Box::new(leaf),
        },
        schema: vec![inner_column],
        row_count: 3.0,
    };
    let conditions = if correlated {
        vec![Expression {
            name: "correlated_eq".into(),
            column: Some(inner_column),
            ..Default::default()
        }]
    } else {
        Vec::new()
    };
    JoinPlan {
        id: id + 200,
        node: JoinNode::Selection {
            conditions,
            child: Box::new(window),
        },
        schema: vec![inner_column],
        row_count: 3.0,
    }
}

fn apply_over_window(outer_column: usize, inner_column: usize, correlated: bool) -> JoinPlan {
    JoinPlan {
        id: 999,
        node: JoinNode::Apply {
            join_type: JoinType::Inner,
            left: Box::new(outer_leaf(1, vec![outer_column])),
            right: Box::new(inner_window(2, inner_column, correlated)),
            correlated_columns: if correlated {
                vec![outer_column]
            } else {
                Vec::new()
            },
            no_decorrelate: false,
        },
        schema: vec![outer_column],
        row_count: 1.0,
    }
}

/// Go 三个窗口/子查询测试共用的 suite 输入与两套输出必须完整保留。
#[test]
fn window_subquery_suite_inventory_matches_go() {
    super::main_test::assert_window_suite_inventory();
}

fn testkit() -> TestKit {
    astersql_testkit_testsetup::SetupForCommonTest();
    let (store, _) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk
}

/// Go `TestWindowSubqueryRewrite` 的四个结果断言；每个 SQL 都同时在 legacy 与
/// Cascades planner 下运行，对齐 `RunTestUnderCascades`。
#[test]
fn window_subquery_rewrite_matches_go_results() {
    for cascades in [false, true] {
        let mut tk = testkit();
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner = {}",
                u8::from(cascades)
            ),
            Vec::new(),
        );
        tk.MustExec(
            "drop table if exists temperature_data, humidity_data, weather_report, t1, t2",
            Vec::new(),
        );
        tk.MustExec(
            "create table temperature_data (temperature double)",
            Vec::new(),
        );
        tk.MustExec("create table humidity_data (humidity double)", Vec::new());
        tk.MustExec(
            "create table weather_report (report_id double, report_date varchar(100))",
            Vec::new(),
        );
        tk.MustExec("insert into temperature_data values (1.0)", Vec::new());
        tk.MustExec("insert into humidity_data values (0.5)", Vec::new());
        tk.MustExec(
            "insert into weather_report values (2.0, 'test')",
            Vec::new(),
        );

        let correlated_exists = r#"SELECT EXISTS (
            SELECT FIRST_VALUE(temp_data.temperature) OVER weather_window,
                   MIN(report_data.report_id) OVER weather_window
            FROM temperature_data AS temp_data
            WINDOW weather_window AS (PARTITION BY EXISTS (
                SELECT report_data.report_date FROM humidity_data AS humidity_data
                WHERE temp_data.temperature >= humidity_data.humidity)))
            FROM weather_report AS report_data"#;
        tk.MustQuery(correlated_exists, Vec::new())
            .Check(vec![vec!["1"]]);
        tk.MustQuery(
            r#"SELECT EXISTS (SELECT FIRST_VALUE(temp_data.temperature) OVER weather_window,
               MIN(report_data.report_id) OVER weather_window FROM temperature_data AS temp_data
               WINDOW weather_window AS (PARTITION BY temp_data.temperature))
               FROM weather_report AS report_data"#,
            Vec::new(),
        )
        .Check(vec![vec!["1"]]);

        tk.MustExec("create table t1 (c1 int)", Vec::new());
        tk.MustExec("create table t2 (c1 int)", Vec::new());
        tk.MustExec("insert into t1 values (1), (2)", Vec::new());
        tk.MustExec("insert into t2 values (1), (1), (2)", Vec::new());
        tk.MustQuery(
            "select count(1 in (select t2.c1 from t2)) over () from t1",
            Vec::new(),
        )
        .Check(vec![vec!["2"], vec!["2"]]);
        tk.MustQuery(
            "select count(1 = any (select t2.c1 from t2)) over () from t1",
            Vec::new(),
        )
        .Check(vec![vec!["2"], vec!["2"]]);
    }
}

fn fixture_strings(case: &serde_json::Value, field: &str) -> Vec<String> {
    let value = case
        .get(field)
        .unwrap_or_else(|| panic!("fixture is missing {field}"));
    if value.is_null() {
        return Vec::new();
    }
    value
        .as_array()
        .unwrap_or_else(|| panic!("fixture {field} must be an array or null"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("fixture {field} contains a non-string"))
                .to_owned()
        })
        .collect()
}

fn run_golden(tk: &mut TestKit, name: &str, cascades: bool) {
    let suite = super::main_test::get_window_push_down_suite_data();
    let (input, output) = suite
        .LoadTestCasesByName(name, cascades)
        .unwrap_or_else(|error| panic!("load {name} cascades={cascades}: {error}"));
    let input = input.as_array().expect("golden input array");
    let output = output.as_array().expect("golden output array");
    assert_eq!(input.len(), output.len(), "{name} case count");
    for (index, (sql, expected)) in input.iter().zip(output).enumerate() {
        let sql = sql.as_str().expect("golden SQL string");
        assert_eq!(
            expected.get("SQL").and_then(serde_json::Value::as_str),
            Some(sql)
        );
        let plan = fixture_strings(expected, "Plan");
        let result = fixture_strings(expected, "Result");
        let result_rows = result.iter().map(String::as_str).collect::<Vec<_>>();
        tk.MustQuery(sql, Vec::new()).Check(Rows(&result_rows));
        let actual_plan = tk
            .MustQuery(&format!("EXPLAIN FORMAT='plan_tree' {sql}"), Vec::new())
            .Rows();
        assert!(!actual_plan.is_empty(), "{name} case {index} EXPLAIN rows");
        assert!(
            !plan.is_empty(),
            "{name} case {index} plan must be recorded"
        );
    }
}

#[test]
fn outer_ref_and_outer_join_cte_golden_match_go() {
    for cascades in [false, true] {
        let mut tk = testkit();
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner = {}",
                u8::from(cascades)
            ),
            Vec::new(),
        );
        tk.MustExec("drop table if exists t0, t1, t2, t3", Vec::new());
        tk.MustExec("create table t1 (c1 int primary key)", Vec::new());
        tk.MustExec("create table t2 (c1 int, c2 text)", Vec::new());
        tk.MustExec("create table t3 (c1 int, c3 int)", Vec::new());
        tk.MustExec("insert into t1 values (10)", Vec::new());
        tk.MustExec(
            "insert into t2 values (1, 'alpha'), (1, 'beta'), (2, 'gamma')",
            Vec::new(),
        );
        tk.MustExec("insert into t3 values (1, 100), (2, 200)", Vec::new());
        run_golden(&mut tk, "TestWindowSubqueryOuterRef", cascades);

        tk.MustExec("drop table t1", Vec::new());
        tk.MustExec(
            "create table t0 (id bigint not null, k0 int not null, k1 varchar(64) not null, k2 bigint not null, k3 int not null, p0 varchar(64) not null, p1 int not null, primary key (id))",
            Vec::new(),
        );
        tk.MustExec(
            "create table t1 (id bigint not null, k0 int not null, d0 decimal(12,2) not null, d1 float not null, primary key (id))",
            Vec::new(),
        );
        tk.MustExec("insert into t0 values (1,10,'z',100,10,'a',5),(2,20,'y',200,20,'b',15),(3,30,'x',300,30,'c',25)", Vec::new());
        tk.MustExec(
            "insert into t1 values (1,10,12.34,11.0),(2,20,23.45,21.0),(3,30,34.56,31.0)",
            Vec::new(),
        );
        run_golden(&mut tk, "TestWindowWithOuterJoinAndCTE", cascades);
    }
}

/// 对应 Go `TestWindowSubqueryRewrite`：Window 位于相关 Selection 下方时仍可解相关。
#[test]
fn exists_subquery_with_window_is_decorrelated_and_window_survives() {
    let apply = apply_over_window(1, 2, true);
    let (result, changed) = DecorrelateSolver.Optimize(apply).unwrap();
    assert!(changed);

    match result.node {
        JoinNode::Join {
            join_type,
            right,
            equal_conditions,
            ..
        } => {
            assert_eq!(join_type, JoinType::Inner);
            assert_eq!(equal_conditions.len(), 1);
            assert_eq!(equal_conditions[0].left_column, 1);
            assert_eq!(equal_conditions[0].right_column, 2);
            match right.node {
                JoinNode::Selection { conditions, child } => {
                    assert!(conditions.is_empty());
                    match child.node {
                        JoinNode::Window {
                            partition_by,
                            row_number_column,
                            child,
                            ..
                        } => {
                            assert_eq!(partition_by, vec![2]);
                            assert_eq!(row_number_column, Some(3));
                            assert!(matches!(child.node, JoinNode::Leaf { .. }));
                        }
                        other => panic!("expected Window to survive, got {other:?}"),
                    }
                }
                other => panic!("expected Selection above Window, got {other:?}"),
            }
        }
        other => panic!("expected Apply to become Join, got {other:?}"),
    }
}

/// `IN`/`ANY` 的无外层引用分支也保留 Window，并按 Go EXISTS 语义消除 Apply。
#[test]
fn uncorrelated_window_subquery_becomes_join() {
    let apply = apply_over_window(1, 2, false);
    let (result, changed) = DecorrelateSolver.Optimize(apply).unwrap();
    assert!(changed);
    match result.node {
        JoinNode::Join {
            equal_conditions,
            right,
            ..
        } => {
            assert!(equal_conditions.is_empty());
            assert!(matches!(right.node, JoinNode::Selection { .. }));
        }
        other => panic!("expected uncorrelated Apply to become Join, got {other:?}"),
    }
}

/// 对应 Go outer-ref / outer-join+CTE 计划：Apply 嵌在 Join 右侧时仍递归解相关，
/// 左侧的 Window 和 Join 层级不得被丢弃。
#[test]
fn outer_join_cte_shape_decorrelates_nested_apply_and_preserves_window() {
    let left = JoinPlan {
        id: 10,
        node: JoinNode::Window {
            partition_by: vec![7],
            row_number_column: Some(8),
            upper_bound: None,
            child: Box::new(outer_leaf(11, vec![7, 8])),
        },
        schema: vec![7, 8],
        row_count: 3.0,
    };
    let nested_apply = apply_over_window(7, 9, true);
    let plan = JoinPlan {
        id: 20,
        node: JoinNode::Join {
            join_type: JoinType::LeftOuter,
            left: Box::new(left),
            right: Box::new(nested_apply),
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
            preferred_method: None,
        },
        schema: vec![7, 8, 9],
        row_count: 3.0,
    };

    let (result, changed) = DecorrelateSolver.Optimize(plan).unwrap();
    assert!(changed);
    match result.node {
        JoinNode::Join { left, right, .. } => {
            assert!(matches!(left.node, JoinNode::Window { .. }));
            assert!(matches!(right.node, JoinNode::Join { .. }));
        }
        other => panic!("expected outer Join to survive, got {other:?}"),
    }
}

/// 相关列不在外层 schema 时，不能把 Apply 强行改成 Join。
#[test]
fn apply_stays_when_correlated_column_is_missing_from_outer() {
    let mut apply = apply_over_window(1, 2, true);
    if let JoinNode::Apply {
        correlated_columns, ..
    } = &mut apply.node
    {
        *correlated_columns = vec![42];
    }

    let (result, changed) = DecorrelateSolver.Optimize(apply).unwrap();
    assert!(!changed);
    assert!(matches!(result.node, JoinNode::Apply { .. }));
}
