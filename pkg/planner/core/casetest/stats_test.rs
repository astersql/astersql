// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 统计信息（Stats）相关 casetest：GroupNDV 与列统计收集路径。
//
// Go 原版通过 Cascades + testdata 校验聚合/连接输入的 GroupNDV 与 explain brief；
// 当前窄运行时改为直连 `collect_column_stats_usage`，验证聚合计划上的谓词列与访问表。
//
// GroupNDV：一组列上的联合基数（Number of Distinct Values）；用于代价估算与 NDV 传递。
// ANALYZE：收集表/列直方图与基数等统计信息，供 CBO（基于代价的优化）使用。

// casetest 如何构造统计数据、运行 planner 预处理/逻辑优化，并检查 GroupNDV 与 explain 结果。
//
// StatsGroupNDVCase 对应 Go 中匿名输出结构，保存每条 SQL 的聚合输入和 join 输入 GroupNDV。
// pub struct StatsGroupNDVCase {
//     pub SQL: String,
//     pub AggInput: String,
//     pub JoinInput: String,
// }
//
// StatsPlanCase 对应 Go 中匿名输出结构，保存 explain brief 的计划行。
// pub struct StatsPlanCase {
//     pub SQL: String,
//     pub Plan: Vec<String>,
// }
//
// TestGroupNDVs 对应 Go 测试：对 testdata 中的 SQL 建逻辑计划、派生统计信息，并抽取 aggregation/join 输入的 GroupNDV。
// #[test]
// pub fn TestGroupNDVs() {
//     let t = testing::T::new();
//     testkit::RunTestUnderCascades(&t, |t: &testing::T, testKit: &testkit::TestKit, cascades: String, caller: String| {
//         testKit.MustExec("use test");
//         testKit.MustExec("drop table if exists t1, t2");
//         testKit.MustExec("create table t1(a int not null, b int not null, key(a,b))");
//         testKit.MustExec("insert into t1 values(1,1),(1,2),(2,1),(2,2),(1,1)");
//         testKit.MustExec("create table t2(a int not null, b int not null, key(a,b))");
//         testKit.MustExec("insert into t2 values(1,1),(1,2),(1,3),(2,1),(2,2),(2,3),(3,1),(3,2),(3,3),(1,1)");
//         testKit.MustExec("analyze table t1");
//         testKit.MustExec("analyze table t2");
//
//         let ctx = context::Background();
//         let p = parser::New();
//         let mut input: Vec<String> = vec![];
//         let mut output: Vec<StatsGroupNDVCase> = vec![];
//         let mut statsSuiteData = GetStatsSuiteData();
//         statsSuiteData.LoadTestCases(t, &mut input, &mut output, &[cascades.clone(), caller.clone()]);
//         for (i, tt) in input.iter().enumerate() {
//             let comment = fmt::Sprintf("case:%v sql: %s", vec![i.to_string(), tt.clone()]);
//             let (stmt, err) = p.ParseOneStmt(tt, "", "");
//             require::NoError(t, err, comment.clone());
//             let mut ret = core::PreprocessorReturn::default();
//             let nodeW = resolve::NewNodeW(stmt);
//             let err = core::Preprocess(context::Background(), testKit.Session(), nodeW.clone(), core::WithPreprocessorReturn(&mut ret));
//             require::NoError(t, err);
//             testKit.Session().GetSessionVars().PlanColumnID.Store(0);
//             let (mut builder, _) = core::NewPlanBuilder().Init(testKit.Session().GetPlanCtx(), ret.InfoSchema, hint::NewQBHintHandler(None));
//             let (mut plan, err) = builder.Build(ctx.clone(), nodeW);
//             require::NoError(t, err, comment.clone());
//             let (optimized, err) = core::LogicalOptimizeTest(ctx.clone(), builder.GetOptFlag() | rule::FlagCollectPredicateColumnsPoint, plan.as_logical());
//             require::NoError(t, err, comment.clone());
//             plan = optimized;
//
// Go 用显式栈遍历逻辑计划，找到第一个 aggregation 和 join；DataSource 触底后回到右侧子树。
//             let mut lp = plan.as_logical();
//             let mut agg: Option<logicalop::LogicalAggregation> = None;
//             let mut join: Option<logicalop::LogicalJoin> = None;
//             let mut stack: Vec<base::LogicalPlan> = Vec::with_capacity(2);
//             let mut traversed = false;
//             while !traversed {
//                 match lp.kind() {
//                     logicalop::Kind::LogicalAggregation => {
//                         agg = Some(lp.clone().downcast());
//                         lp = lp.Children()[0].clone();
//                     }
//                     logicalop::Kind::LogicalJoin => {
//                         let v: logicalop::LogicalJoin = lp.clone().downcast();
//                         join = Some(v.clone());
//                         lp = v.Children()[0].clone();
//                         stack.push(v.Children()[1].clone());
//                     }
//                     logicalop::Kind::LogicalApply => {
//                         let v: logicalop::LogicalApply = lp.clone().downcast();
//                         lp = v.Children()[0].clone();
//                         stack.push(v.Children()[1].clone());
//                     }
//                     logicalop::Kind::LogicalUnionAll => {
//                         let v: logicalop::LogicalUnionAll = lp.clone().downcast();
//                         lp = v.Children()[0].clone();
//                         for child in v.Children().iter().skip(1) {
//                             stack.push(child.clone());
//                         }
//                     }
//                     logicalop::Kind::DataSource => {
//                         if stack.is_empty() {
//                             traversed = true;
//                         } else {
//                             lp = stack.remove(0);
//                         }
//                     }
//                     _ => {
//                         lp = lp.Children()[0].clone();
//                     }
//                 }
//             }
//
//             let mut aggInput = String::new();
//             let mut joinInput = String::new();
//             if let Some(agg) = agg {
//                 let s = core::GetStats4Test(agg.Children()[0].clone());
//                 aggInput = property::ToString(s.GroupNDVs);
//             }
//             if let Some(join) = join {
//                 let l = core::GetStats4Test(join.Children()[0].clone());
//                 let r = core::GetStats4Test(join.Children()[1].clone());
//                 joinInput = format!("{};{}", property::ToString(l.GroupNDVs), property::ToString(r.GroupNDVs));
//             }
//             testdata::OnRecord(|| {
//                 output[i].SQL = tt.clone();
//                 output[i].AggInput = aggInput.clone();
//                 output[i].JoinInput = joinInput.clone();
//             });
//             require::Equal(t, output[i].AggInput.clone(), aggInput, comment.clone());
//             require::Equal(t, output[i].JoinInput.clone(), joinInput, comment);
//         }
//     });
// }
//
// TestNDVGroupCols 对应 Go 测试：通过 explain brief 校验聚合和 join 的行数估算，重点是 NDV group column 的传递。
// #[test]
// pub fn TestNDVGroupCols() {
//     let t = testing::T::new();
//     testkit::RunTestUnderCascades(&t, |t: &testing::T, testKit: &testkit::TestKit, cascades: String, caller: String| {
//         testKit.MustExec("use test");
//         testKit.MustExec("drop table if exists t1, t2");
//         testKit.MustExec("create table t1(a int not null, b int not null, key(a,b))");
//         testKit.MustExec("insert into t1 values(1,1),(1,2),(2,1),(2,2)");
//         testKit.MustExec("create table t2(a int not null, b int not null, key(a,b))");
//         testKit.MustExec("insert into t2 values(1,1),(1,2),(1,3),(2,1),(2,2),(2,3),(3,1),(3,2),(3,3)");
//         testKit.MustExec("analyze table t1");
//         testKit.MustExec("analyze table t2");
//
// Go 原注释：默认 RPC encoding 可能导致 statistics explain 结果不同，从而使测试不稳定。
//         testKit.MustExec("set @@tidb_enable_chunk_rpc = on");
//
//         let mut input: Vec<String> = vec![];
//         let mut output: Vec<StatsPlanCase> = vec![];
//         let mut statsSuiteData = GetStatsSuiteData();
//         statsSuiteData.LoadTestCases(t, &mut input, &mut output, &[cascades.clone(), caller.clone()]);
//         for (i, tt) in input.iter().enumerate() {
//             testdata::OnRecord(|| {
//                 output[i].SQL = tt.clone();
//                 output[i].Plan = testdata::ConvertRowsToStrings(testKit.MustQuery(format!("explain format = 'brief' {}", tt)).Rows());
//             });
// Go 测试点是 aggregation 和 join 的 row count estimation。
//             testKit.MustQuery(format!("explain format = 'brief' {}", tt)).Check(testkit::Rows(output[i].Plan.clone()));
//         }
//     });
// }
// */
use std::collections::{BTreeMap, BTreeSet};

use crate::main_test::load_suite;
use astersql_parser::Parser;
use astersql_planner_core_rule::collect_column_stats_usage::collect_column_stats_usage;
use astersql_planner_core_rule::rule_init::{
    AggKind, AggregateExpr, Expr, FieldType, JoinType, Plan, PlanKind,
};

/// 构造 DataSource→Aggregation 计划，断言 `collect_column_stats_usage` 收集分组/聚合列。
#[test]
fn group_ndv_inputs_collect_group_and_aggregate_columns() {
    // 列表达式工厂：带有符号整数类型。
    let column = |id| Expr::Column {
        id,
        field_type: FieldType::SignedInt,
    };
    // 叶节点：表 9，索引 (1,2)，唯一键与估算行数。
    let source = Plan {
        kind: PlanKind::DataSource {
            table_id: 9,
            indexes: BTreeMap::from([(1, vec![1, 2])]),
            partition: None,
            selected_partitions: None,
        },
        schema: vec![1, 2],
        children: Vec::new(),
        predicates: Vec::new(),
        keys: vec![vec![1, 2]],
        estimated_rows: 100.0,
        used_stats: BTreeMap::new(),
    };
    // 聚合：COUNT(DISTINCT col2) GROUP BY col1，对应 Go 侧 GroupNDV 输入列收集。
    let aggregate = Plan {
        kind: PlanKind::Aggregation {
            aggregates: vec![AggregateExpr {
                kind: AggKind::Count,
                args: vec![column(2)],
                distinct: true,
            }],
            group_by: vec![column(1)],
        },
        schema: vec![1, 2],
        children: vec![source],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    };
    // 第二个参数 true：按「需要统计」路径收集谓词/访问列。
    let usage = collect_column_stats_usage(&aggregate, true);
    assert_eq!(
        usage.predicate_columns,
        BTreeMap::from([(1, false), (2, false)])
    );
    assert_eq!(usage.visited_tables, BTreeSet::from([9]));
}

/// Go TestGroupNDVs also walks a join and records both input sides.  Keep this
/// assertion separate from aggregation so one missing traversal branch cannot
/// be masked by the group-by case.
#[test]
fn group_ndv_inputs_collect_both_join_sides_and_equal_columns() {
    let column = |id| Expr::Column {
        id,
        field_type: FieldType::SignedInt,
    };
    let source = |table_id, id| Plan {
        kind: PlanKind::DataSource {
            table_id,
            indexes: BTreeMap::from([(1, vec![id])]),
            partition: None,
            selected_partitions: None,
        },
        schema: vec![id],
        children: Vec::new(),
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    };
    let join = Plan {
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: vec![Expr::Scalar {
                function: "eq".into(),
                args: vec![column(1), column(2)],
                field_type: FieldType::Bool,
            }],
            other_conditions: Vec::new(),
        },
        schema: vec![1, 2],
        children: vec![source(11, 1), source(22, 2)],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    };
    let usage = collect_column_stats_usage(&join, true);
    assert_eq!(usage.visited_tables, BTreeSet::from([11, 22]));
    assert_eq!(
        usage.predicate_columns,
        BTreeMap::from([(1, false), (2, false)])
    );
    assert_eq!(usage.interesting_columns[&11], BTreeSet::from([1]));
    assert_eq!(usage.interesting_columns[&22], BTreeSet::from([2]));
}

/// Connect both Go stats tests to the real input/output fixture boundary and
/// keep every golden field that the Go assertions consume.
#[test]
fn stats_suite_cases_preserve_go_inputs_and_golden_outputs() {
    let cases = [("TestGroupNDVs", 25), ("TestNDVGroupCols", 14)];
    for cascades in [false, true] {
        let suite = load_suite("stats_suite", cascades);
        for (name, expected_len) in cases {
            let (input, output) = suite
                .LoadTestCasesByName(name, cascades)
                .unwrap_or_else(|error| panic!("load stats_suite/{name}: {error}"));
            let input = input.as_array().expect("stats input must be an array");
            let output = output.as_array().expect("stats output must be an array");
            assert_eq!(input.len(), expected_len, "{name} input coverage changed");
            assert_eq!(output.len(), expected_len, "{name} output coverage changed");

            for (index, (sql, golden)) in input.iter().zip(output).enumerate() {
                let sql = sql.as_str().unwrap_or_else(|| {
                    panic!("stats_suite/{name} case {index} input is not SQL: {sql}")
                });
                Parser::default()
                    .Parse(sql, "", "")
                    .unwrap_or_else(|error| {
                        panic!("stats_suite/{name} case {index} SQL {sql:?}: {error}")
                    });
                assert_eq!(golden["SQL"].as_str(), Some(sql), "{name} case {index}");

                match name {
                    "TestGroupNDVs" => {
                        assert!(golden["AggInput"].is_string(), "{name} case {index}");
                        assert!(golden["JoinInput"].is_string(), "{name} case {index}");
                    }
                    "TestNDVGroupCols" => {
                        let plan = golden["Plan"]
                            .as_array()
                            .unwrap_or_else(|| panic!("{name} case {index} Plan is not an array"));
                        assert!(!plan.is_empty(), "{name} case {index} has no plan rows");
                        assert!(
                            plan.iter()
                                .all(|row| row.as_str().is_some_and(|row| !row.is_empty())),
                            "{name} case {index} has an invalid plan row"
                        );
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}
