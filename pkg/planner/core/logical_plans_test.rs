// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 逻辑计划（logical plan）回归测试，对应 Go `logical_plans_test.go`。
//
// 通过 `main_test` 的 JSON fixture 与优化规则标志位，验证谓词下推、
// 列裁剪、聚合消除、Join 重排、分区处理、窗口函数等逻辑优化结果，
// 以及部分物理计划（physical plan）字符串形态。

use base_dependency::PhysicalPlan as _;
use base_dependency::Plan as _;
use expression_dependency as expression;
use logicalop_dependency as logicalop;
use logicalop_dependency::LogicalPlan as _;
use physicalop_dependency as physicalop;
use rule_dependency as rule;

use super::main_test::{
    FixtureExpected, build_runtime_for_test, logical_optimize_default_for_test,
    logical_optimize_for_test, logical_optimize_partition_fixture_for_test,
    logical_optimize_with_agg_pushdown_for_test, logical_plan_string,
    optimize_query_without_post_with_window_concurrency_for_test, plan_fixture,
};

/// 按 Go 用例名返回显式优化规则标志组合；`None` 表示使用 PlanBuilder 默认标志。
fn fixture_flags(name: &str) -> Option<u64> {
    let prune = rule::FLAG_PRUNE_COLUMNS | rule::FLAG_PRUNE_COLUMNS_AGAIN;
    Some(match name {
        "TestEagerAggregation" => {
            rule::FLAG_BUILD_KEY_INFO
                | rule::FLAG_PREDICATE_PUSH_DOWN
                | prune
                | rule::FLAG_PUSH_DOWN_AGG
        }
        "TestPlanBuilder" => prune,
        "TestPredicatePushDown" => {
            rule::FLAG_CONVERT_OUTER_TO_INNER_JOIN
                | rule::FLAG_PREDICATE_PUSH_DOWN
                | rule::FLAG_DECORRELATE
                | prune
                | rule::FLAG_PREDICATE_SIMPLIFICATION
        }
        "TestSubquery" => {
            rule::FLAG_BUILD_KEY_INFO
                | rule::FLAG_DECORRELATE
                | prune
                | rule::FLAG_SEMI_JOIN_REWRITE
        }
        "TestUniqueKeyInfo" => {
            rule::FLAG_PREDICATE_PUSH_DOWN | rule::FLAG_PRUNE_COLUMNS | rule::FLAG_BUILD_KEY_INFO
        }
        "TestAggPrune" => {
            rule::FLAG_PREDICATE_PUSH_DOWN
                | prune
                | rule::FLAG_BUILD_KEY_INFO
                | rule::FLAG_ELIMINATE_AGG
                | rule::FLAG_ELIMINATE_PROJECTION
        }
        "TestColumnPruning" => rule::FLAG_PREDICATE_PUSH_DOWN | prune,
        "TestSortByItemsPruning" => {
            rule::FLAG_ELIMINATE_PROJECTION | rule::FLAG_PREDICATE_PUSH_DOWN | prune
        }
        "TestDeriveNotNullConds"
        | "TestJoinPredicatePushDown"
        | "TestOuterWherePredicatePushDown" => {
            rule::FLAG_PREDICATE_PUSH_DOWN | rule::FLAG_DECORRELATE | prune
        }
        "TestJoinReOrder" => rule::FLAG_PREDICATE_PUSH_DOWN | rule::FLAG_JOIN_REORDER,
        "TestSimplifyOuterJoin" => {
            rule::FLAG_PREDICATE_PUSH_DOWN | prune | rule::FLAG_CONVERT_OUTER_TO_INNER_JOIN
        }
        "TestTablePartition" => {
            rule::FLAG_PREDICATE_PUSH_DOWN | prune | rule::FLAG_PARTITION_PROCESSOR
        }
        "TestTopNPushDown"
        | "TestUnion"
        | "TestWindowFunction"
        | "TestWindowParallelFunction"
        | "TestOuterJoinEliminator" => return None,
        "TestPruneColumnsForDelete" => prune,
        _ => panic!("missing explicit Go optimizer flags for {name}"),
    })
}

/// 用 fixture 对应标志跑逻辑优化，返回上下文与逻辑计划。
fn optimize_fixture(
    name: &str,
    sql: &str,
) -> Result<(base_dependency::ContextRef, logicalop::LogicalPlanRef), String> {
    match fixture_flags(name) {
        Some(flags) if name == "TestEagerAggregation" => {
            logical_optimize_with_agg_pushdown_for_test(sql, flags)
        }
        Some(flags) => logical_optimize_for_test(sql, flags),
        None => logical_optimize_default_for_test(sql),
    }
}

/// 分区表 fixture：带 PARTITION_PROCESSOR 等标志做逻辑优化。
fn optimize_partition_fixture(
    sql: &str,
    info_schema_index: usize,
) -> Result<(base_dependency::ContextRef, logicalop::LogicalPlanRef), String> {
    logical_optimize_partition_fixture_for_test(
        sql,
        info_schema_index,
        rule::FLAG_DECORRELATE
            | rule::FLAG_PRUNE_COLUMNS
            | rule::FLAG_PRUNE_COLUMNS_AGAIN
            | rule::FLAG_PREDICATE_PUSH_DOWN
            | rule::FLAG_PARTITION_PROCESSOR,
    )
}

/// 深度优先查找第一个 LogicalJoin。
fn find_join(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalJoin> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalJoin>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_join(child.as_ref()))
        })
}

/// 深度优先查找第一个 LogicalSelection。
fn find_selection(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalSelection> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalSelection>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_selection(child.as_ref()))
        })
}

/// 收集计划树中全部 DataSource。
fn collect_sources<'a>(
    plan: &'a dyn logicalop::LogicalPlan,
    sources: &mut Vec<&'a logicalop::DataSource>,
) {
    if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
        sources.push(source);
    }
    for child in plan.Children() {
        collect_sources(child.as_ref(), sources);
    }
}

/// 收集列裁剪检查关心的节点（DataSource / UnionAll / Limit）。
fn collect_column_pruning_nodes<'a>(
    plan: &'a dyn logicalop::LogicalPlan,
    nodes: &mut Vec<&'a dyn logicalop::LogicalPlan>,
) {
    if plan.as_any().is::<logicalop::DataSource>()
        || plan.as_any().is::<logicalop::LogicalUnionAll>()
        || plan.as_any().is::<logicalop::LogicalLimit>()
    {
        nodes.push(plan);
    }
    for child in plan.Children() {
        collect_column_pruning_nodes(child.as_ref(), nodes);
    }
}

/// 将表达式列表格式化为与 Go Stringify 一致的字符串。
fn expression_list(
    plan: &dyn logicalop::LogicalPlan,
    expressions: &[expression::ExprBox],
) -> String {
    let context = plan
        .SCtx()
        .expect("optimized fixture plan retains context")
        .GetExprCtx()
        .GetEvalCtx();
    expression::StringifyExpressionsWithCtx(context, expressions)
}

/// 对字符串形态期望的 fixture 组逐条断言逻辑计划 ToString。
fn assert_string_plan_group(name: &str) {
    let fixture = plan_fixture(name);
    for (index, (case, expected)) in fixture.cases.iter().zip(&fixture.expected).enumerate() {
        let FixtureExpected::Plan(expected) = expected else {
            panic!("{name} case {index} is not a string-plan expectation: {expected:?}");
        };
        match optimize_fixture(name, case.sql()) {
            Ok((_, plan)) => assert_eq!(
                expected,
                &logical_plan_string(plan.as_ref()),
                "{name} case {index}: {}",
                case.sql()
            ),
            Err(error) => assert_eq!(expected, &error, "{name} case {index}: {}", case.sql()),
        }
    }
}

/// 将物理计划树格式化为窗口/分区用例期望的 ToString 风格。
fn physical_window_plan_string(plan: &dyn base_dependency::PhysicalPlan) -> String {
    let children = plan
        .children()
        .into_iter()
        .map(physical_window_plan_string)
        .collect::<Vec<_>>();
    if let Some(reader) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableReader>()
    {
        return format!(
            "TableReader({})",
            reader
                .TablePlan
                .as_deref()
                .map(physical_window_plan_string)
                .unwrap_or_default()
        );
    }
    if let Some(reader) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalIndexReader>()
    {
        return format!(
            "IndexReader({})",
            reader
                .IndexPlan
                .as_deref()
                .map(physical_window_plan_string)
                .unwrap_or_default()
        );
    }
    if let Some(reader) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalIndexLookUpReader>()
    {
        let index = reader
            .IndexPlan
            .as_deref()
            .map(physical_window_plan_string)
            .unwrap_or_default();
        let table = reader
            .TablePlan
            .as_deref()
            .map(physical_window_plan_string)
            .unwrap_or_default();
        return format!("IndexLookUp({index}, {table})");
    }
    // HashJoin 命名跟随 build 侧：InnerChildIdx==0 为 RightHashJoin。
    if let Some(join) = plan.as_any().downcast_ref::<physicalop::PhysicalHashJoin>() {
        let side = if join.BasePhysicalJoin.InnerChildIdx == 0 {
            "RightHashJoin"
        } else {
            "LeftHashJoin"
        };
        let parameters = plan.s_ctx().GetExprCtx().GetEvalCtx();
        let keys = join
            .EqualConditions
            .iter()
            .filter(|condition| condition.GetArgs().len() == 2)
            .map(|condition| {
                format!(
                    "({},{})",
                    condition.GetArgs()[0].StringWithCtx(Some(parameters as _), "OFF"),
                    condition.GetArgs()[1].StringWithCtx(Some(parameters as _), "OFF")
                )
            })
            .collect::<String>();
        return format!("{side}{{{}}}{keys}", children.join("->"));
    }
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableScan>()
    {
        let table = scan
            .Table
            .as_ref()
            .map(|table| table.Name.O.as_str())
            .unwrap_or("unknown");
        return format!("Table({table})");
    }
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalIndexScan>()
    {
        let table = scan
            .Table
            .as_ref()
            .map(|table| table.Name.O.as_str())
            .unwrap_or("unknown");
        let index = scan
            .Index
            .as_ref()
            .map(|index| index.Name.O.as_str())
            .unwrap_or("unknown");
        let ranges = scan
            .Ranges
            .iter()
            .map(|range| range.String())
            .collect::<Vec<_>>()
            .join(",");
        return format!("Index({table}.{index})[{ranges}]");
    }
    let current = if plan.as_any().is::<physicalop::PhysicalProjection>() {
        "Projection".to_owned()
    } else if plan.as_any().is::<physicalop::PhysicalSort>() {
        "Sort".to_owned()
    } else if let Some(window) = plan.as_any().downcast_ref::<physicalop::PhysicalWindow>() {
        format!("Window({})", window.ExplainInfo())
    } else if let Some(shuffle) = plan.as_any().downcast_ref::<physicalop::PhysicalShuffle>() {
        format!("Partition({})", shuffle.ExplainInfo())
    } else if plan.as_any().is::<physicalop::PhysicalStreamAgg>() {
        "StreamAgg".to_owned()
    } else if plan.as_any().is::<physicalop::PhysicalHashAgg>() {
        "HashAgg".to_owned()
    } else if let Some(selection) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalSelection>()
    {
        format!("Sel([{}])", selection.ExplainInfo())
    } else if plan.as_any().is::<physicalop::PhysicalMaxOneRow>() {
        "MaxOneRow".to_owned()
    } else if plan.as_any().is::<physicalop::PhysicalApply>() {
        return format!("Apply{{{}}}", children.join("->"));
    } else {
        plan.tp(&[])
    };
    if children.is_empty() {
        current
    } else {
        format!("{}->{current}", children.join("->"))
    }
}

/// 窗口函数 fixture：跳过 post-optimize，可配置窗口并发度。
fn assert_window_plan_group(name: &str) {
    let fixture = plan_fixture(name);
    let window_concurrency = if name == "TestWindowParallelFunction" {
        4
    } else {
        1
    };
    for (index, (case, expected)) in fixture.cases.iter().zip(&fixture.expected).enumerate() {
        let FixtureExpected::Plan(expected) = expected else {
            panic!("{name} case {index} is not a string-plan expectation: {expected:?}");
        };
        match optimize_query_without_post_with_window_concurrency_for_test(
            case.sql(),
            window_concurrency,
        ) {
            Ok(plan) => {
                let actual = physical_window_plan_string(plan.as_ref());
                assert_eq!(expected, &actual, "{name} case {index}: {}", case.sql());
            }
            Err(error) => assert_eq!(expected, &error, "{name} case {index}: {}", case.sql()),
        }
    }
}

/// 在独立 statement context 中构建窗口计划并返回完整 Go 对齐字符串。
fn window_plan_string_for_test(sql: &str, window_concurrency: usize) -> Result<String, String> {
    optimize_query_without_post_with_window_concurrency_for_test(sql, window_concurrency)
        .map(|plan| physical_window_plan_string(plan.as_ref()))
}

/// PlanBuilder 用例：逻辑计划列裁剪后 ToString，或 DML/Explain 包装形态。
fn plan_builder_result_string(sql: &str) -> Result<String, String> {
    let (_, _, built) = super::main_test::build_plan_builder_fixture_for_test(sql)?;
    match built {
        crate::BuiltRuntimePlan::Logical(mut plan) => {
            crate::LogicalOptimizeForTest(
                rule::FLAG_PRUNE_COLUMNS | rule::FLAG_PRUNE_COLUMNS_AGAIN,
                &mut plan,
            )
            .map_err(|error| format!("logical optimize {sql:?}: {error}"))?;
            Ok(logical_plan_string(plan.as_ref()))
        }
        crate::BuiltRuntimePlan::NonLogical(plan) => {
            if plan.as_any().is::<crate::RuntimeExplain>() {
                return Ok("*core.Explain".to_owned());
            }
            if let Some(update) = plan.as_any().downcast_ref::<physicalop::Update>() {
                return Ok(format!(
                    "{}->Update",
                    physical_window_plan_string(update.SelectPlan.as_ref())
                ));
            }
            if let Some(delete) = plan.as_any().downcast_ref::<physicalop::Delete>() {
                return Ok(format!(
                    "{}->Delete",
                    physical_window_plan_string(delete.SelectPlan.as_ref())
                ));
            }
            if let Some(insert) = plan.as_any().downcast_ref::<physicalop::Insert>() {
                let source = insert
                    .SelectPlan
                    .as_ref()
                    .ok_or_else(|| format!("build {sql:?}: INSERT has no SELECT source"))?;
                return Ok(format!(
                    "{}->Insert",
                    physical_window_plan_string(source.as_ref())
                ));
            }
            Ok(plan.tp(&[]))
        }
    }
}

/// 断言 TestPlanBuilder 全部用例。
fn assert_plan_builder_group() {
    let fixture = plan_fixture("TestPlanBuilder");
    for (index, (case, expected)) in fixture.cases.iter().zip(&fixture.expected).enumerate() {
        let FixtureExpected::Plan(expected) = expected else {
            panic!("TestPlanBuilder case {index} has wrong expectation: {expected:?}");
        };
        match plan_builder_result_string(case.sql()) {
            Ok(actual) => assert_eq!(expected, &actual, "case {index}: {}", case.sql()),
            Err(error) => assert_eq!(expected, &error, "case {index}: {}", case.sql()),
        }
    }
}

/// 生成仅比对逻辑计划字符串的测试函数。
macro_rules! string_plan_fixture_test {
    ($test:ident, $fixture:literal) => {
        #[test]
        fn $test() {
            assert_string_plan_group($fixture);
        }
    };
}

string_plan_fixture_test!(test_eager_aggregation_fixture, "TestEagerAggregation");
#[test]
/// PlanBuilder 构建与轻量逻辑优化结果。
fn test_plan_builder_fixture() {
    assert_plan_builder_group();
}
string_plan_fixture_test!(test_subquery_fixture, "TestSubquery");
string_plan_fixture_test!(test_top_n_push_down_fixture, "TestTopNPushDown");
#[test]
/// 串行窗口函数物理计划形态。
fn test_window_function_fixture() {
    assert_window_plan_group("TestWindowFunction");
}

#[test]
/// 并行窗口（concurrency=4）物理计划形态。
fn test_window_parallel_function_fixture() {
    let fixture = plan_fixture("TestWindowParallelFunction");
    let case = fixture.cases[1].sql().to_owned();
    let FixtureExpected::Plan(expected) = &fixture.expected[1] else {
        panic!("parallel window stability case has a non-plan expectation")
    };

    for iteration in 0..4 {
        assert_eq!(
            expected,
            &window_plan_string_for_test(&case, 4)
                .unwrap_or_else(|error| panic!("repeat {iteration}: {error}")),
            "repeat {iteration}: {case}"
        );
    }

    std::thread::scope(|scope| {
        let builds = (0..8)
            .map(|_| scope.spawn(|| window_plan_string_for_test(&case, 4)))
            .collect::<Vec<_>>();
        for (worker, build) in builds.into_iter().enumerate() {
            assert_eq!(
                expected,
                &build
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                    .unwrap_or_else(|error| panic!("worker {worker}: {error}")),
                "worker {worker}: {case}"
            );
        }
    });

    assert_window_plan_group("TestWindowParallelFunction");
}

string_plan_fixture_test!(test_agg_prune_fixture, "TestAggPrune");
#[test]
/// 分区表逻辑优化与 PartitionProcessor 结果。
fn test_table_partition_fixture() {
    let fixture = plan_fixture("TestTablePartition");
    for (index, (case, expected)) in fixture.cases.iter().zip(&fixture.expected).enumerate() {
        let super::main_test::FixtureCase::TablePartition(case) = case else {
            panic!("partition case {index} has wrong input type")
        };
        let FixtureExpected::Plan(expected) = expected else {
            panic!("partition case {index} has wrong expected type")
        };
        let (_, plan) = optimize_partition_fixture(&case.sql, case.info_schema_index)
            .unwrap_or_else(|error| panic!("partition case {index}: {error}"));
        assert_eq!(
            expected,
            &logical_plan_string(plan.as_ref()),
            "partition case {index}"
        );
    }
}
string_plan_fixture_test!(test_join_reorder_fixture, "TestJoinReOrder");
string_plan_fixture_test!(
    test_outer_join_eliminator_fixture,
    "TestOuterJoinEliminator"
);

#[test]
/// 谓词下推（Predicate Push Down）逻辑计划。
fn test_predicate_push_down() {
    assert_string_plan_group("TestPredicatePushDown");
}

#[test]
/// UNION 用例同时校验是否报错与最优计划字符串。
fn union_fixture_matches_error_and_plan_fields() {
    let fixture = plan_fixture("TestUnion");
    for (index, (case, expected)) in fixture.cases.iter().zip(&fixture.expected).enumerate() {
        let FixtureExpected::BestError(expected) = expected else {
            panic!("TestUnion case {index} has wrong expected type");
        };
        let actual = optimize_fixture(&fixture.name, case.sql());
        assert_eq!(
            expected.error,
            actual.is_err(),
            "TestUnion case {index}: {:?}",
            actual.as_ref().err()
        );
        if let Ok((_, plan)) = actual {
            assert_eq!(
                expected.best,
                logical_plan_string(plan.as_ref()),
                "case {index}"
            );
        }
    }
}

#[test]
/// 导出 NOT NULL 与 Join 两侧下推谓词的细粒度字段断言。
fn predicate_detail_fixtures_match_exact_fields() {
    for name in ["TestDeriveNotNullConds", "TestJoinPredicatePushDown"] {
        let fixture = plan_fixture(name);
        for (index, (case, expected)) in fixture.cases.iter().zip(&fixture.expected).enumerate() {
            let (_, plan) = optimize_fixture(name, case.sql())
                .unwrap_or_else(|error| panic!("{name} case {index}: {error}"));
            let join =
                find_join(plan.as_ref()).unwrap_or_else(|| panic!("{name} case {index}: join"));
            let mut left_sources = Vec::new();
            let mut right_sources = Vec::new();
            collect_sources(join.Children()[0].as_ref(), &mut left_sources);
            collect_sources(join.Children()[1].as_ref(), &mut right_sources);
            let left = expression_list(plan.as_ref(), &left_sources[0].PushedDownConds);
            let right = expression_list(plan.as_ref(), &right_sources[0].PushedDownConds);
            match expected {
                FixtureExpected::DeriveNotNull(expected) => {
                    assert_eq!(
                        expected.plan,
                        logical_plan_string(plan.as_ref()),
                        "case {index}"
                    );
                    assert_eq!(expected.left, left, "case {index} left");
                    assert_eq!(expected.right, right, "case {index} right");
                }
                FixtureExpected::JoinPredicates(expected) => {
                    assert_eq!(expected.left, left, "case {index} left");
                    assert_eq!(expected.right, right, "case {index} right");
                }
                other => panic!("{name} case {index}: wrong expectation {other:?}"),
            }
        }
    }
}

#[test]
/// 外连接 WHERE 下推谓词与简化后的 JoinType 断言。
fn outer_predicate_and_join_type_fixtures_match_exact_fields() {
    for name in ["TestOuterWherePredicatePushDown", "TestSimplifyOuterJoin"] {
        let fixture = plan_fixture(name);
        for (index, (case, expected)) in fixture.cases.iter().zip(&fixture.expected).enumerate() {
            let (_, plan) = optimize_fixture(name, case.sql())
                .unwrap_or_else(|error| panic!("{name} case {index}: {error}"));
            let join =
                find_join(plan.as_ref()).unwrap_or_else(|| panic!("{name} case {index}: join"));
            match expected {
                FixtureExpected::OuterPredicates(expected) => {
                    let selection = find_selection(plan.as_ref())
                        .unwrap_or_else(|| panic!("{name} case {index}: selection"));
                    let mut left_sources = Vec::new();
                    let mut right_sources = Vec::new();
                    collect_sources(join.Children()[0].as_ref(), &mut left_sources);
                    collect_sources(join.Children()[1].as_ref(), &mut right_sources);
                    assert_eq!(
                        expected.selection,
                        expression_list(plan.as_ref(), &selection.Conditions)
                    );
                    assert_eq!(
                        expected.left,
                        expression_list(plan.as_ref(), &left_sources[0].PushedDownConds)
                    );
                    assert_eq!(
                        expected.right,
                        expression_list(plan.as_ref(), &right_sources[0].PushedDownConds)
                    );
                }
                FixtureExpected::SimplifyOuter(expected) => {
                    assert_eq!(
                        expected.best,
                        logical_plan_string(plan.as_ref()),
                        "case {index}"
                    );
                    assert_eq!(
                        expected.join_type,
                        join.JoinType.to_string(),
                        "case {index}"
                    );
                }
                other => panic!("{name} case {index}: wrong expectation {other:?}"),
            }
        }
    }
}

#[test]
/// 列裁剪后 DataSource 列集，以及 Sort/TopN 排序列裁剪。
fn column_and_sort_pruning_fixtures_match_exact_fields() {
    for name in ["TestColumnPruning", "TestSortByItemsPruning"] {
        let fixture = plan_fixture(name);
        for (index, (case, expected)) in fixture.cases.iter().zip(&fixture.expected).enumerate() {
            let (_, plan) = optimize_fixture(name, case.sql())
                .unwrap_or_else(|error| panic!("{name} case {index}: {error}"));
            match expected {
                FixtureExpected::ColumnPruning(expected) => {
                    let mut sources = Vec::new();
                    collect_column_pruning_nodes(plan.as_ref(), &mut sources);
                    let columns = sources
                        .into_iter()
                        .map(|source| {
                            (
                                source.ID() as usize,
                                source
                                    .Schema()
                                    .Columns
                                    .iter()
                                    .map(|column| column.OrigName.clone())
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect::<std::collections::BTreeMap<_, _>>();
                    assert_eq!(expected.data_sources, columns, "{name} case {index}");
                }
                FixtureExpected::SortColumns(expected) => {
                    fn find_sort_items(
                        plan: &dyn logicalop::LogicalPlan,
                    ) -> Option<&[planner_util_dependency::ByItems]> {
                        if let Some(sort) = plan.as_any().downcast_ref::<logicalop::LogicalSort>() {
                            return Some(&sort.ByItems);
                        }
                        if let Some(top_n) = plan.as_any().downcast_ref::<logicalop::LogicalTopN>()
                        {
                            return Some(&top_n.ByItems);
                        }
                        plan.Children()
                            .iter()
                            .find_map(|child| find_sort_items(child.as_ref()))
                    }
                    let items = find_sort_items(plan.as_ref())
                        .unwrap_or_else(|| panic!("{name} case {index}: missing Sort/TopN"));
                    let columns = items
                        .iter()
                        .flat_map(|item| expression::ExtractColumns(item.Expr.as_ref()))
                        .map(|column| column.OrigName.clone())
                        .collect::<Vec<_>>();
                    assert_eq!(expected, &columns, "{name} case {index}");
                }
                other => panic!("{name} case {index}: wrong expectation {other:?}"),
            }
        }
    }
}

#[test]
/// 唯一键信息传播，以及 DELETE 列裁剪后的输出布局与内部物理计划。
fn unique_key_and_delete_fixtures_are_typed_and_reach_real_planner_paths() {
    for name in ["TestUniqueKeyInfo", "TestPruneColumnsForDelete"] {
        let fixture = plan_fixture(name);
        for (index, (case, expected)) in fixture.cases.iter().zip(&fixture.expected).enumerate() {
            match expected {
                FixtureExpected::UniqueKey(expected) => {
                    let (_, plan) = optimize_fixture(name, case.sql())
                        .unwrap_or_else(|error| panic!("{name} case {index}: {error}"));
                    // 按节点 ID 收集 Schema 上的 PK/UK 列名组合。
                    fn collect_keys(
                        plan: &dyn logicalop::LogicalPlan,
                        keys: &mut std::collections::BTreeMap<usize, Vec<Vec<String>>>,
                    ) {
                        keys.insert(
                            plan.ID() as usize,
                            plan.Schema()
                                .PKOrUK
                                .iter()
                                .map(|key| {
                                    key.iter().map(|column| column.OrigName.clone()).collect()
                                })
                                .collect(),
                        );
                        for child in plan.Children() {
                            collect_keys(child.as_ref(), keys);
                        }
                    }
                    let mut actual = std::collections::BTreeMap::new();
                    collect_keys(plan.as_ref(), &mut actual);
                    assert_eq!(expected.nodes, actual, "case {index} unique keys");
                }
                FixtureExpected::PruneDelete(expected) => {
                    let (_, _, built) = super::main_test::build_runtime_for_test(case.sql())
                        .unwrap_or_else(|error| panic!("{name} case {index}: {error}"));
                    let crate::BuiltRuntimePlan::NonLogical(plan) = built else {
                        panic!("{name} case {index}: DELETE must build a non-logical plan")
                    };
                    let delete = plan
                        .as_any()
                        .downcast_ref::<physicalop_dependency::Delete>()
                        .unwrap_or_else(|| panic!("{name} case {index}: expected Delete wrapper"));
                    let pruned_output = delete
                        .output_names()
                        .0
                        .iter()
                        .enumerate()
                        .map(|(offset, name)| {
                            format!(
                                "{}: {offset}",
                                name.as_ref().map_or_else(String::new, |name| name.String())
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let full_layout_info = delete
                        .TblColPosInfos
                        .iter()
                        .map(|layout| {
                            let Some(indexes) = &layout.IndexesRowLayout else {
                                return vec!["no column-pruning happened".to_owned()];
                            };
                            let mut rows = vec![
                                format!(
                                    "tid: {}, [start, end]: [{}, {}] ",
                                    layout.TblID, layout.Start, layout.End
                                ),
                                format!(
                                    "handle cols: {}:{}",
                                    layout
                                        .HandleCols
                                        .iter()
                                        .map(|column| column.OrigName.clone())
                                        .collect::<Vec<_>>()
                                        .join(","),
                                    layout
                                        .HandleCols
                                        .iter()
                                        .map(|column| column.Index.to_string())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                            ];
                            for index_layout in indexes {
                                assert_eq!(
                                    indexes
                                        .Get(index_layout.ID)
                                        .map(|layout| layout.Name.as_str()),
                                    Some(index_layout.Name.as_str()),
                                    "case {index} index layout must be addressable by ID"
                                );
                                rows.push(format!(
                                    "index {}: {} ",
                                    index_layout.Name,
                                    index_layout.Columns.join(" ")
                                ));
                                rows.push(format!(
                                    "col offset: [{}]",
                                    index_layout
                                        .Offsets
                                        .iter()
                                        .map(usize::to_string)
                                        .collect::<Vec<_>>()
                                        .join(" ")
                                ));
                            }
                            rows
                        })
                        .collect::<Vec<_>>();
                    // DELETE 内部 SelectPlan 的物理 ToString（含 PointGet 等）。
                    fn physical_types(plan: &dyn base_dependency::PhysicalPlan) -> String {
                        if let Some(point) = plan
                            .as_any()
                            .downcast_ref::<physicalop_dependency::PointGetPlan>()
                        {
                            let table = point
                                .TblInfo
                                .as_ref()
                                .map_or("", |table| table.Name.O.as_str());
                            let handle = point
                                .AccessColumns
                                .first()
                                .and_then(|column| column.OrigName.rsplit('.').next())
                                .expect("point get must expose its actual handle column");
                            return format!(
                                "PointGet(Handle({table}.{handle}){})",
                                point.Handle.unwrap_or_default()
                            );
                        }
                        if let Some(batch) = plan
                            .as_any()
                            .downcast_ref::<physicalop_dependency::BatchPointGetPlan>()
                        {
                            let table = batch
                                .PointGetPlan
                                .TblInfo
                                .as_ref()
                                .map_or("", |table| table.Name.O.as_str());
                            let handle = batch
                                .PointGetPlan
                                .AccessColumns
                                .first()
                                .and_then(|column| column.OrigName.rsplit('.').next())
                                .expect("batch point get must expose its actual handle column");
                            return format!(
                                "BatchPointGet(Handle({table}.{handle})[{}])",
                                batch
                                    .Handles
                                    .iter()
                                    .map(i64::to_string)
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            );
                        }
                        if let Some(scan) = plan
                            .as_any()
                            .downcast_ref::<physicalop_dependency::PhysicalTableScan>()
                        {
                            let table = scan
                                .Table
                                .as_ref()
                                .map_or("unknown", |table| table.Name.O.as_str());
                            return format!("Table({table})");
                        }
                        if let Some(selection) =
                            plan.as_any()
                                .downcast_ref::<physicalop_dependency::PhysicalSelection>()
                        {
                            let eval = selection
                                .PhysicalSchemaProducer
                                .BasePhysicalPlan
                                .s_ctx()
                                .GetExprCtx()
                                .GetEvalCtx();
                            let conditions = selection
                                .Conditions
                                .iter()
                                .map(|condition| condition.ExplainInfo(eval))
                                .collect::<Vec<_>>()
                                .join(", ");
                            let child = plan
                                .children()
                                .first()
                                .map_or_else(String::new, |child| physical_types(*child));
                            return format!("{child}->Sel([{conditions}])");
                        }
                        if plan
                            .as_any()
                            .downcast_ref::<physicalop_dependency::PhysicalProjection>()
                            .is_some()
                        {
                            let child = plan
                                .children()
                                .first()
                                .map_or_else(String::new, |child| physical_types(*child));
                            return format!("{child}->Projection");
                        }
                        if let Some(reader) =
                            plan.as_any()
                                .downcast_ref::<physicalop_dependency::PhysicalTableReader>()
                        {
                            return format!(
                                "TableReader({})",
                                reader
                                    .GetTablePlan()
                                    .map_or_else(String::new, physical_types)
                            );
                        }
                        if let Some(join) = plan
                            .as_any()
                            .downcast_ref::<physicalop_dependency::PhysicalHashJoin>()
                        {
                            // Go ToString 按 build 侧位置命名 HashJoin，
                            // Go's ToString names a hash join by the build-side
                            // 与语义 JoinType 无关。
                            // position, independently of its semantic JoinType.
                            let name = if join.BasePhysicalJoin.InnerChildIdx == 0 {
                                "RightHashJoin"
                            } else {
                                "LeftHashJoin"
                            };
                            let children = plan
                                .children()
                                .into_iter()
                                .map(physical_types)
                                .collect::<Vec<_>>()
                                .join("->");
                            let left = join
                                .BasePhysicalJoin
                                .LeftJoinKeys
                                .iter()
                                .map(|column| column.OrigName.clone())
                                .collect::<Vec<_>>()
                                .join(",");
                            let right = join
                                .BasePhysicalJoin
                                .RightJoinKeys
                                .iter()
                                .map(|column| column.OrigName.clone())
                                .collect::<Vec<_>>()
                                .join(",");
                            return format!("{name}{{{children}}}({left},{right})");
                        }
                        if plan
                            .as_any()
                            .downcast_ref::<physicalop_dependency::PhysicalUnionAll>()
                            .is_some()
                        {
                            let children = plan
                                .children()
                                .into_iter()
                                .map(physical_types)
                                .collect::<Vec<_>>()
                                .join("->");
                            return format!("PartitionUnionAll{{{children}}}");
                        }
                        let children = plan.children();
                        if children.is_empty() {
                            return plan.tp(&[]);
                        }
                        format!(
                            "{}{{{}}}",
                            plan.tp(&[]),
                            children
                                .into_iter()
                                .map(physical_types)
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    }
                    assert_eq!(expected.pruned_output, pruned_output, "case {index} output");
                    assert_eq!(
                        expected.full_layout_info, full_layout_info,
                        "case {index} layout"
                    );
                    assert_eq!(
                        expected.inside_plan,
                        physical_types(delete.SelectPlan.as_ref()),
                        "case {index} inside plan"
                    );
                }
                other => panic!("{name} case {index}: wrong expectation {other:?}"),
            }
        }
    }
}

#[test]
fn go_merge_187_mpp_cte_site_optimizer_retains_shuffle_evidence_fields() {
    let fixture = plan_fixture("TestWindowParallelFunction");
    let sql = fixture
        .cases
        .iter()
        .zip(&fixture.expected)
        .find_map(|(case, expected)| {
            matches!(expected, FixtureExpected::Plan(plan) if plan.contains("Partition("))
                .then(|| case.sql())
        })
        .expect("parallel window fixture must contain a shuffle");
    let plan = optimize_query_without_post_with_window_concurrency_for_test(sql, 4).unwrap();
    let tree = super::FlattenTypedPhysicalPlan(plan.as_ref()).unwrap();
    let shuffle = tree
        .iter()
        .find_map(|op| {
            op.Origin
                .as_any()
                .downcast_ref::<physicalop::PhysicalShuffle>()
        })
        .unwrap();
    assert_eq!(shuffle.DataSources.len(), 1);
    assert_eq!(shuffle.ByItemArrays.len(), 1);
    assert!(!shuffle.ByItemArrays[0].is_empty());
    assert_eq!(
        shuffle.SplitterType,
        physicalop::physical_shuffle::PartitionSplitterType::Hash
    );
    assert!(
        tree.iter()
            .any(|op| op.Origin.id() == shuffle.DataSources[0].id())
    );
}
