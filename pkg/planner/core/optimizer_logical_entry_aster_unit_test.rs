// Copyright 2026 AsterSQL.

// 逻辑优化入口规则的迁移期单元测试。
//
// 覆盖生成列替换、结果稳定排序、半连接改写、聚合消除、常量传播、
// Join 键类型转换、外连接消除等逻辑优化规则，并通过对逻辑计划树的
// 查找辅助函数构造断言目标。

use expression_dependency::Expression as _;
use logicalop_dependency as logicalop;
use logicalop_dependency::LogicalPlan as _;

use super::main_test::{
    PlannerTestStatsHandle, build_logical_for_test, logical_optimize_for_test,
    logical_optimize_with_stats_handle_for_test, logical_plan_string,
};

/// 深度优先查找首个 LogicalJoin。
fn find_join(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalJoin> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalJoin>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_join(child.as_ref()))
        })
}

/// 将首个 Join 标为 PreferCorrelate 半连接，供相关化用例使用。
fn mark_first_join_prefer_correlate(plan: &mut dyn logicalop::LogicalPlan) -> bool {
    if let Some(join) = plan.as_any_mut().downcast_mut::<logicalop::LogicalJoin>() {
        join.PreferCorrelate = true;
        join.JoinType = logicalop::JoinType::SemiJoin;
        return true;
    }
    plan.Children_mut()
        .iter_mut()
        .any(|child| mark_first_join_prefer_correlate(child.as_mut()))
}

/// 将首个 Projection 中标量函数改名为全文匹配，供 FTS 规则测试。
fn mark_first_projection_function_as_fts(plan: &mut dyn logicalop::LogicalPlan) -> bool {
    if let Some(projection) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalProjection>()
    {
        for expression in &mut projection.Exprs {
            if let Some(function) = expression
                .as_any_mut()
                .downcast_mut::<expression_dependency::ScalarFunction>()
            {
                function.FuncName = crate::ast::NewCIStr("fts_match_word");
                return true;
            }
        }
    }
    plan.Children_mut()
        .iter_mut()
        .any(|child| mark_first_projection_function_as_fts(child.as_mut()))
}

/// 计划树是否包含 Dual。
fn contains_dual(plan: &dyn logicalop::LogicalPlan) -> bool {
    plan.as_any().is::<logicalop::LogicalTableDual>()
        || plan
            .Children()
            .iter()
            .any(|child| contains_dual(child.as_ref()))
}

/// 统计计划树中某具体类型节点个数。
fn count_type<T: 'static>(plan: &dyn logicalop::LogicalPlan) -> usize {
    usize::from(plan.as_any().is::<T>())
        + plan
            .Children()
            .iter()
            .map(|child| count_type::<T>(child.as_ref()))
            .sum::<usize>()
}

/// 查找首个 DataSource。
fn find_source(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::DataSource> {
    plan.as_any()
        .downcast_ref::<logicalop::DataSource>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_source(child.as_ref()))
        })
}

/// 可变查找首个 DataSource。
fn find_source_mut(plan: &mut dyn logicalop::LogicalPlan) -> Option<&mut logicalop::DataSource> {
    if plan.as_any().is::<logicalop::DataSource>() {
        return plan.as_any_mut().downcast_mut::<logicalop::DataSource>();
    }
    plan.Children_mut()
        .iter_mut()
        .find_map(|child| find_source_mut(child.as_mut()))
}

/// 查找首个 LogicalProjection。
fn find_projection(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalProjection> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_projection(child.as_ref()))
        })
}

/// 查找首个 LogicalSort。
fn find_sort(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalSort> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalSort>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_sort(child.as_ref()))
        })
}

/// 在 Projection 中查找标量函数表达式。
fn find_projection_scalar<'a>(
    plan: &'a dyn logicalop::LogicalPlan,
    name: &str,
) -> Option<&'a expression_dependency::ScalarFunction> {
    if let Some(projection) = plan.as_any().downcast_ref::<logicalop::LogicalProjection>()
        && let Some(function) = projection
            .Exprs
            .iter()
            .filter_map(|expression| expression.as_scalar_function())
            .find(|function| function.FuncName.L == name)
    {
        return Some(function);
    }
    plan.Children()
        .iter()
        .find_map(|child| find_projection_scalar(child.as_ref(), name))
}

/// 为测试安装带索引的虚拟生成列。
fn install_indexed_virtual_c(
    plan: &mut dyn logicalop::LogicalPlan,
    virtual_expression: expression_dependency::ExprBox,
) -> i64 {
    let generated_type = virtual_expression
        .GetType(
            plan.SCtx()
                .expect("initialized plan")
                .GetExprCtx()
                .GetEvalCtx(),
        )
        .clone();
    let source = find_source_mut(plan).expect("query data source");
    let generated_offset = source
        .TableInfo
        .Columns
        .iter()
        .position(|column| column.Name.L == "c")
        .expect("indexed c column");
    source.TableInfo.Columns[generated_offset].GeneratedExprString = "a + 1".to_owned();
    source.TableInfo.Columns[generated_offset].FieldType = generated_type.clone();
    source.Columns[generated_offset].GeneratedExprString = "a + 1".to_owned();
    source.Columns[generated_offset].FieldType = generated_type.clone();
    source.Schema_mut().Columns[generated_offset].RetType = Some(generated_type);
    source.Schema_mut().Columns[generated_offset].VirtualExpr = Some(virtual_expression);
    source.Schema().Columns[generated_offset].UniqueID
}

#[test]
/// 生成列替换应使用已索引的虚拟表达式。
fn generated_column_substitution_uses_indexed_virtual_expression() {
    let (_, mut plan) =
        build_logical_for_test("select a + 1 from t").expect("generated-column query plan");
    let virtual_expression = find_projection(plan.as_ref())
        .expect("query projection")
        .Exprs[0]
        .CloneExpr();
    let generated_unique_id = install_indexed_virtual_c(plan.as_mut(), virtual_expression);

    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_GC_SUBSTITUTE,
        &mut plan,
    )
    .expect("generated-column substitution");

    let projection = find_projection(plan.as_ref()).expect("query projection remains");
    let substituted = projection.Exprs[0]
        .as_column()
        .expect("indexed virtual expression is replaced by its generated column");
    assert_eq!(substituted.UniqueID, generated_unique_id);
}

#[test]
/// 生成列替换应改写安全谓词与排序项。
fn generated_column_substitution_rewrites_safe_predicates_and_sort_items() {
    let (_, mut predicate_plan) = build_logical_for_test("select c from t where a + 1 > 2")
        .expect("generated-column predicate plan");
    let predicate_expression = find_selection(predicate_plan.as_ref())
        .expect("selection")
        .Conditions[0]
        .as_scalar_function()
        .expect("comparison")
        .GetArgs()[0]
        .CloneExpr();
    let predicate_column = install_indexed_virtual_c(predicate_plan.as_mut(), predicate_expression);
    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_GC_SUBSTITUTE,
        &mut predicate_plan,
    )
    .expect("predicate substitution");
    let predicate_argument = find_selection(predicate_plan.as_ref())
        .expect("selection remains")
        .Conditions[0]
        .as_scalar_function()
        .expect("comparison remains")
        .GetArgs()[0]
        .as_column()
        .expect("comparison operand substituted");
    assert_eq!(predicate_argument.UniqueID, predicate_column);

    let (_, mut sort_plan) = build_logical_for_test("select c from t order by a + 1")
        .expect("generated-column sort plan");
    let sort_expression = find_sort(sort_plan.as_ref()).expect("logical sort").ByItems[0]
        .Expr
        .CloneExpr();
    let sort_column = install_indexed_virtual_c(sort_plan.as_mut(), sort_expression);
    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_GC_SUBSTITUTE,
        &mut sort_plan,
    )
    .expect("sort substitution");
    assert_eq!(
        find_sort(sort_plan.as_ref())
            .expect("logical sort remains")
            .ByItems[0]
            .Expr
            .as_column()
            .expect("sort item substituted")
            .UniqueID,
        sort_column
    );
}

#[test]
/// 结果稳定化应注入基于句柄的确定性顺序。
fn stabilize_results_injects_deterministic_handle_order() {
    let (_, mut plan) =
        build_logical_for_test("select a, b from t").expect("unordered result plan");
    assert!(find_sort(plan.as_ref()).is_none());
    let handle_unique_id = find_source(plan.as_ref())
        .expect("data source")
        .GetPKIsHandleCol()
        .expect("integer primary-key handle")
        .UniqueID;

    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_STABILIZE_RESULTS,
        &mut plan,
    )
    .expect("result stabilization");

    let sort = find_sort(plan.as_ref()).expect("deterministic sort is injected");
    assert_eq!(sort.ByItems.len(), 1);
    assert_eq!(
        sort.ByItems[0]
            .Expr
            .as_column()
            .expect("sorts by primary-key handle")
            .UniqueID,
        handle_unique_id
    );
    assert!(sort.Children()[0].as_any().is::<logicalop::DataSource>());
}

#[test]
/// 半连接改写应得到内连接+分组与外侧投影。
fn semi_join_rewrite_builds_inner_join_grouping_and_outer_projection() {
    let (_, mut plan) = build_logical_for_test(
        "select * from t where exists (select /*+ SEMI_JOIN_REWRITE() */ * from t2 where t2.a = t.a)",
    )
    .expect("hinted EXISTS plan");
    let apply = find_apply(plan.as_ref()).expect("semi apply before decorrelation");
    let semi = &apply.LogicalJoin;
    assert_eq!(semi.JoinType, logicalop::JoinType::SemiJoin);
    assert_ne!(
        semi.PreferJoinType & u64::from(hint_dependency::PreferRewriteSemiJoin),
        0
    );
    let projection_count = count_type::<logicalop::LogicalProjection>(plan.as_ref());

    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_DECORRELATE | rule_dependency::FLAG_SEMI_JOIN_REWRITE,
        &mut plan,
    )
    .expect("semi join rewrite");

    assert_eq!(
        count_type::<logicalop::LogicalProjection>(plan.as_ref()),
        projection_count + 1,
        "rewrite adds an outer-schema projection"
    );
    let join = find_join(plan.as_ref()).expect("rewritten inner join");
    assert_eq!(join.JoinType, logicalop::JoinType::InnerJoin);
    assert_eq!(join.Children().len(), 2);
    let aggregation = join.Children()[1]
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
        .expect("right side grouped by semi-join keys");
    assert_eq!(aggregation.GroupByItems.len(), join.EqualConditions.len());
    assert!(
        aggregation
            .AggFuncs
            .iter()
            .all(|function| function.Name.eq_ignore_ascii_case("firstrow"))
    );
}

#[test]
/// 唯一键分组的聚合应被投影替代。
fn aggregation_elimination_replaces_unique_key_group_with_projection() {
    let (_, mut plan) = build_logical_for_test("select max(b) from t group by a")
        .expect("unique-key aggregation plan");
    plan.BuildKeyInfo();
    assert_eq!(
        count_type::<logicalop::LogicalAggregation>(plan.as_ref()),
        1
    );
    let projections = count_type::<logicalop::LogicalProjection>(plan.as_ref());
    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_ELIMINATE_AGG,
        &mut plan,
    )
    .expect("aggregation elimination");
    assert_eq!(
        count_type::<logicalop::LogicalAggregation>(plan.as_ref()),
        0
    );
    assert_eq!(
        count_type::<logicalop::LogicalProjection>(plan.as_ref()),
        projections + 1
    );
}

#[test]
/// 聚合消除须保留可空列 COUNT 语义。
fn aggregation_elimination_preserves_nullable_count_semantics() {
    let (_, mut plan) = build_logical_for_test("select count(e) from t group by a")
        .expect("nullable count aggregation plan");
    plan.BuildKeyInfo();
    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_ELIMINATE_AGG,
        &mut plan,
    )
    .expect("nullable count elimination");
    let conditional = find_projection_scalar(plan.as_ref(), crate::ast::If)
        .expect("COUNT(nullable) becomes IF(ISNULL(arg),0,1)");
    assert_eq!(conditional.GetArgs().len(), 3);
    assert_eq!(
        conditional.GetArgs()[0]
            .as_scalar_function()
            .expect("count null check")
            .FuncName
            .L,
        crate::ast::IsNull
    );
    assert!(conditional.GetArgs()[1].as_constant().is_some());
    assert!(conditional.GetArgs()[2].as_constant().is_some());
}

#[test]
/// 倾斜 distinct 聚合应构建两级分组。
fn skew_distinct_aggregation_builds_two_level_grouping() {
    let (_, mut plan) = build_logical_for_test("select a, count(distinct b) from t group by a")
        .expect("skew distinct aggregation plan");
    assert_eq!(
        count_type::<logicalop::LogicalAggregation>(plan.as_ref()),
        1
    );
    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_SKEW_DISTINCT_AGG,
        &mut plan,
    )
    .expect("skew distinct rewrite");
    assert_eq!(
        count_type::<logicalop::LogicalAggregation>(plan.as_ref()),
        2
    );
    let top = find_aggregation(plan.as_ref()).expect("top aggregation");
    assert!(top.AggFuncs.iter().all(|function| !function.HasDistinct));
    let bottom = top.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
        .expect("bottom aggregation");
    assert_eq!(bottom.GroupByItems.len(), 2);
}

#[test]
/// Max/Min 消除应加非空过滤、排序与 Limit。
fn max_min_elimination_adds_null_filter_order_and_limit() {
    let (_, mut plan) = build_logical_for_test("select max(e) from t").expect("scalar max plan");
    assert!(find_sort(plan.as_ref()).is_none());
    assert!(find_limit(plan.as_ref()).is_none());
    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_PRUNE_COLUMNS | rule_dependency::FLAG_MAX_MIN_ELIMINATE,
        &mut plan,
    )
    .expect("max elimination");
    let rendered = logical_plan_string(plan.as_ref());
    let sort =
        find_sort(plan.as_ref()).unwrap_or_else(|| panic!("MAX orders its argument: {rendered}"));
    assert!(sort.ByItems[0].Desc);
    assert_eq!(
        find_limit(plan.as_ref()).expect("MAX reads one row").Count,
        1
    );
    let selection = find_selection(plan.as_ref()).expect("nullable MAX filters NULL");
    assert_eq!(
        selection.Conditions[0]
            .as_scalar_function()
            .expect("NOT NULL predicate")
            .FuncName
            .L,
        crate::ast::UnaryNot
    );
}

#[test]
/// 常量传播应将派生表谓词上提到 Join 之上。
fn constant_propagation_pulls_derived_table_predicate_above_join() {
    let (_, mut plan) = build_logical_for_test(
        "select t.a from t join (select a from t2 where a > 1) s on t.a = s.a",
    )
    .expect("derived-table constant predicate plan");
    let selections = count_type::<logicalop::LogicalSelection>(plan.as_ref());
    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_CONSTANT_PROPAGATION,
        &mut plan,
    )
    .expect("cross-block constant propagation");
    assert_eq!(
        count_type::<logicalop::LogicalSelection>(plan.as_ref()),
        selections + 1
    );
    let join = find_join(plan.as_ref()).expect("join remains");
    let pulled = plan
        .Children()
        .iter()
        .find_map(|child| find_selection(child.as_ref()))
        .or_else(|| find_selection(plan.as_ref()))
        .expect("candidate predicate above join");
    assert!(pulled.Conditions.iter().all(|condition| {
        expression_dependency::ExtractColumns(condition.as_ref())
            .iter()
            .all(|column| join.Schema().Contains(column))
    }));
}

#[test]
/// Join 键类型转换应恢复整型键并保护文本侧。
fn join_key_type_cast_restores_integer_key_and_guards_text_side() {
    let (_, mut plan) = build_logical_for_test("select x.a from t x join t y on x.a = y.c_str")
        .expect("mixed join-key plan");
    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_PRUNE_COLUMNS | rule_dependency::FLAG_PREDICATE_PUSH_DOWN,
        &mut plan,
    )
    .expect("materialize implicit cast projections");
    let before = find_join(plan.as_ref()).expect("join");
    assert_eq!(before.OtherConditions.len(), 1);
    assert!(
        before.OtherConditions[0]
            .as_scalar_function()
            .expect("equality")
            .GetArgs()
            .iter()
            .all(|argument| {
                argument
                    .GetType(
                        before
                            .SCtx()
                            .expect("join context")
                            .GetExprCtx()
                            .GetEvalCtx(),
                    )
                    .EvalType()
                    == expression_dependency::types::ETReal
            })
    );
    let selections = count_type::<logicalop::LogicalSelection>(plan.as_ref());
    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_JOIN_KEY_TYPE_CAST,
        &mut plan,
    )
    .expect("join-key type cast rewrite");
    let join = find_join(plan.as_ref()).expect("join remains");
    assert!(
        join.EqualConditions[0]
            .as_scalar_function()
            .expect("rewritten equality")
            .GetArgs()
            .iter()
            .all(|argument| {
                argument
                    .GetType(join.SCtx().expect("join context").GetExprCtx().GetEvalCtx())
                    .EvalType()
                    == expression_dependency::types::ETInt
            })
    );
    assert_eq!(
        count_type::<logicalop::LogicalSelection>(plan.as_ref()),
        selections + 1,
        "text side gets a lossless-cast guard"
    );
}

#[test]
/// 外连接消除应去掉未引用的唯一内侧。
fn outer_join_elimination_removes_unused_unique_inner_side() {
    let (_, mut plan) = build_logical_for_test("select x.a from t x left join t y on x.a = y.a")
        .expect("eliminable outer join plan");
    plan.BuildKeyInfo();
    assert_eq!(count_type::<logicalop::LogicalJoin>(plan.as_ref()), 1);
    super::optimizer_runtime::LogicalOptimizeForTest(
        rule_dependency::FLAG_ELIMINATE_OUTER_JOIN,
        &mut plan,
    )
    .expect("outer join elimination");
    assert_eq!(count_type::<logicalop::LogicalJoin>(plan.as_ref()), 0);
    let source = find_source(plan.as_ref()).expect("outer data source remains");
    assert_eq!(source.TableAsName.as_ref().expect("outer alias").L, "x");
}

/// 查找首个 LogicalAggregation。
fn find_aggregation(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalAggregation> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_aggregation(child.as_ref()))
        })
}

/// 查找首个 LogicalApply。
fn find_apply(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalApply> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalApply>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_apply(child.as_ref()))
        })
}

/// 查找首个 LogicalExpand。
fn find_expand(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalExpand> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalExpand>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_expand(child.as_ref()))
        })
}

/// 查找首个 LogicalUnionAll。
fn find_union(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalUnionAll> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalUnionAll>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_union(child.as_ref()))
        })
}

/// 查找首个 LogicalLimit。
fn find_limit(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalLimit> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalLimit>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_limit(child.as_ref()))
        })
}

/// 查找首个 LogicalTopN。
fn find_top_n(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalTopN> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalTopN>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_top_n(child.as_ref()))
        })
}

/// 查找首个 LogicalSelection。
fn find_selection(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalSelection> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalSelection>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_selection(child.as_ref()))
        })
}

/// 查找首个 LogicalMaxOneRow。
fn find_max_one_row(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalMaxOneRow> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalMaxOneRow>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_max_one_row(child.as_ref()))
        })
}

#[test]
/// TopN 推导与谓词简化应对真实逻辑计划产生可观察改写。
fn derive_top_n_and_predicate_simplification_rules_mutate_real_plans() {
    let (_, derived) = logical_optimize_for_test(
        "select * from (select a, b, row_number() over(order by a) as rn from t) x where rn <= 3",
        rule_dependency::FLAG_PRUNE_COLUMNS
            | rule_dependency::FLAG_ELIMINATE_PROJECTION
            | rule_dependency::FLAG_DERIVE_TOP_N_FROM_WINDOW,
    )
    .expect("window upper-bound plan");
    assert_eq!(
        count_type::<logicalop::LogicalTopN>(derived.as_ref()),
        1,
        "derive-TopN dispatcher must insert the partition TopN below Window: {}",
        logical_plan_string(derived.as_ref())
    );

    let (_, unsimplified) = logical_optimize_for_test("select a from t where a > 1 and true", 0)
        .expect("unsimplified selection");
    let before = find_selection(unsimplified.as_ref())
        .expect("selection before simplification")
        .Conditions
        .len();
    let (_, simplified) = logical_optimize_for_test(
        "select a from t where a > 1 and true",
        rule_dependency::FLAG_PREDICATE_SIMPLIFICATION,
    )
    .expect("simplified selection");
    let after = find_selection(simplified.as_ref())
        .expect("selection after simplification")
        .Conditions
        .len();
    assert!(
        after <= before,
        "predicate simplification must not reintroduce predicates"
    );
}

#[test]
/// 保序 Join 重排应标记索引顺序承载者。
fn order_aware_join_reorder_marks_index_order_carrier() {
    let sql = "select t.f, t.g from t join t t2 on t.a = t2.a order by t.f, t.g limit 5";
    let (_, untouched) = logical_optimize_for_test(sql, 0).expect("unannotated join");
    assert!(
        !find_join(untouched.as_ref())
            .expect("join without order-aware rule")
            .InternalPreferJoinOrder
    );

    let (_, annotated) =
        logical_optimize_for_test(sql, rule_dependency::FLAG_ORDER_AWARE_JOIN_REORDER)
            .expect("order-aware join plan");
    assert!(
        find_join(annotated.as_ref())
            .expect("join with order-aware rule")
            .InternalPreferJoinOrder
    );
}

#[test]
/// 内侧键空值拒绝时，外连接可改写为半连接。
fn outer_join_to_semi_join_rewrites_null_rejected_inner_key() {
    let sql = "select t.a from t left join t t2 on t.a = t2.a where t2.a is null";
    let (_, original) = logical_optimize_for_test(sql, 0).expect("outer-join plan");
    assert_eq!(
        find_join(original.as_ref()).expect("outer join").JoinType,
        logicalop::JoinType::LeftOuterJoin
    );

    let (_, rewritten) =
        logical_optimize_for_test(sql, rule_dependency::FLAG_OUTER_JOIN_TO_SEMI_JOIN)
            .expect("anti-semi rewrite");
    assert_eq!(
        find_join(rewritten.as_ref())
            .expect("anti-semi join")
            .JoinType,
        logicalop::JoinType::AntiSemiJoin
    );
    assert!(find_selection(rewritten.as_ref()).is_none());
}

#[test]
/// 右外连接改写后必须保留原右侧作为 AntiSemi 的输出侧。
fn right_outer_join_to_semi_join_preserves_right_schema() {
    let sql = "select t2.a from t right join t t2 on t.a = t2.a where t.a is null";
    let (_, rewritten) =
        logical_optimize_for_test(sql, rule_dependency::FLAG_OUTER_JOIN_TO_SEMI_JOIN)
            .expect("right anti-semi rewrite");
    let join = find_join(rewritten.as_ref()).expect("anti-semi join");
    assert_eq!(join.JoinType, logicalop::JoinType::AntiSemiJoin);
    assert_eq!(rewritten.Schema().Columns.len(), 1);
    assert_eq!(
        join.Schema().Columns.len(),
        join.Children()[0].Schema().Columns.len()
    );
    assert!(
        join.Schema()
            .Columns
            .iter()
            .zip(&join.Children()[0].Schema().Columns)
            .all(|(output, preserved)| output.UniqueID == preserved.UniqueID)
    );
    assert!(join.Schema().Contains(&rewritten.Schema().Columns[0]));
}

#[test]
/// PreferCorrelate 半连接应被相关化为 Apply。
fn correlate_rewrites_preferred_semi_join_to_apply() {
    let (_, mut plan) = build_logical_for_test("select t.a from t join t t2 on t.a = t2.a")
        .expect("equality join plan");
    crate::LogicalOptimizeForTest(rule_dependency::FLAG_PREDICATE_PUSH_DOWN, &mut plan)
        .expect("classify JOIN ON equality before correlate");
    let original = find_join(plan.as_ref()).expect("equality join");
    assert!(!original.EqualConditions.is_empty());
    assert!(mark_first_join_prefer_correlate(plan.as_mut()));

    crate::LogicalOptimizeForTest(rule_dependency::FLAG_CORRELATE, &mut plan)
        .expect("correlate optimization");
    let apply = find_apply(plan.as_ref()).expect("preferred semi join must become Apply");
    assert_eq!(apply.LogicalJoin.JoinType, logicalop::JoinType::SemiJoin);
    assert_eq!(apply.CorCols.len(), 1);
    assert_eq!(count_type::<logicalop::LogicalLimit>(plan.as_ref()), 1);
}

#[test]
/// Sequence 下推应穿过一元主查询算子靠近数据源。
fn push_down_sequence_moves_sequence_below_unary_main_query() {
    let (context, main_query) = build_logical_for_test("select a from t").expect("main-query plan");
    let mut cte = logicalop::LogicalTableDual {
        RowCount: 1,
        ..logicalop::LogicalTableDual::default()
    }
    .Init(context.clone(), 0);
    cte.SetSchema(expression_dependency::NewSchema(Vec::new()));
    let mut sequence = logicalop::LogicalSequence::default().Init(context, 0);
    sequence.SetChildren(vec![Box::new(cte), main_query]);
    let mut plan: logicalop::LogicalPlanRef = Box::new(sequence);

    crate::LogicalOptimizeForTest(rule_dependency::FLAG_PUSH_DOWN_SEQUENCE, &mut plan)
        .expect("push-down sequence optimization");
    assert!(plan.as_any().is::<logicalop::LogicalProjection>());
    assert_eq!(count_type::<logicalop::LogicalSequence>(plan.as_ref()), 1);
    let projection = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .expect("main projection remains at root");
    assert!(
        projection.Children()[0]
            .as_any()
            .is::<logicalop::LogicalSequence>()
    );
}

#[test]
/// 未解析的全文函数在 Projection 上应被拒绝规则拦截。
fn full_text_reject_rule_rejects_unresolved_projection_function() {
    let (_, mut plan) =
        build_logical_for_test("select a + 1 from t").expect("scalar projection plan");
    assert!(mark_first_projection_function_as_fts(plan.as_mut()));
    let result = crate::LogicalOptimizeForTest(
        rule_dependency::FLAG_FULLTEXT_INDEX_RESOLVE_REJECT,
        &mut plan,
    );
    let error = match result {
        Ok(_) => panic!("unresolved SELECT FTS function must be rejected"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("FTS_MATCH_WORD()"),
        "unexpected error: {error}"
    );
}

#[test]
/// 全文四阶段规则应能解析完整逻辑计划树。
fn full_text_four_phase_rules_resolve_real_logical_tree() {
    let (context, mut plan) =
        build_logical_for_test("select 1 + a as score from t").expect("FTS template plan");
    let projection = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalProjection>()
        .expect("scalar root projection");
    let mut fts = projection.Exprs[0].clone();
    let function = fts
        .as_any_mut()
        .downcast_mut::<expression_dependency::ScalarFunction>()
        .expect("scalar FTS template");
    function.FuncName = crate::ast::NewCIStr("fts_match_word");
    let indexed_column = function.GetArgs()[1]
        .as_column()
        .expect("FTS column argument")
        .clone();
    projection.Exprs[0] = fts.clone();
    let projection_schema = projection.Schema().Clone();
    let projection_names = projection.OutputNames().Shallow();
    let mut source = projection.TakeChildren().remove(0);
    let source_schema = source.Schema().Clone();
    let source_names = source.OutputNames().Shallow();
    let data_source = source
        .as_any_mut()
        .downcast_mut::<logicalop::DataSource>()
        .expect("FTS data source");
    let offset = data_source
        .TableInfo
        .Columns
        .iter()
        .position(|column| column.ID == indexed_column.ID)
        .expect("indexed table column");
    data_source
        .TableInfo
        .Indices
        .push(expression_dependency::model::IndexInfo {
            ID: 9_999,
            Name: crate::ast::NewCIStr("fts_a"),
            Table: data_source.TableInfo.Name.clone(),
            Columns: vec![expression_dependency::model::IndexColumn {
                Name: data_source.TableInfo.Columns[offset].Name.clone(),
                Offset: offset as isize,
                ..expression_dependency::model::IndexColumn::default()
            }],
            State: expression_dependency::model::StatePublic,
            FullTextInfo: Some(expression_dependency::model::FullTextIndexInfo::default()),
            ..expression_dependency::model::IndexInfo::default()
        });

    let mut selection = logicalop::LogicalSelection {
        Conditions: vec![fts.clone()],
        ..logicalop::LogicalSelection::default()
    }
    .Init(context.clone(), 0);
    selection.SetSchema(source_schema.Clone());
    selection.SetOutputNames(source_names.Shallow());
    selection.SetChildren(vec![source]);
    let mut top_n = logicalop::LogicalTopN {
        ByItems: vec![logicalop::ByItems {
            Expr: fts.clone(),
            Desc: true,
        }],
        Count: 5,
        ..logicalop::LogicalTopN::default()
    }
    .Init(context, 0);
    top_n.SetSchema(source_schema);
    top_n.SetOutputNames(source_names);
    top_n.SetChildren(vec![Box::new(selection)]);
    projection.SetSchema(projection_schema);
    projection.SetOutputNames(projection_names);
    projection.SetChildren(vec![Box::new(top_n)]);

    crate::LogicalOptimizeForTest(
        rule_dependency::FLAG_FULLTEXT_INDEX_RESOLVE_WHERE
            | rule_dependency::FLAG_FULLTEXT_INDEX_RESOLVE_TOP_N
            | rule_dependency::FLAG_FULLTEXT_INDEX_RESOLVE_PROJECTION
            | rule_dependency::FLAG_FULLTEXT_INDEX_RESOLVE_REJECT,
        &mut plan,
    )
    .expect("four-phase FTS resolution");

    assert!(find_selection(plan.as_ref()).is_none());
    let source = find_source(plan.as_ref()).expect("resolved FTS source");
    let push_down = source.FtsPushDown.as_ref().expect("FTS push-down metadata");
    assert_eq!(
        push_down.QueryInfo.QueryType,
        logicalop::FTSQueryType::WithScore
    );
    assert_eq!(push_down.QueryInfo.TopK, 5);
    assert_eq!(
        source.Schema().Columns.last().expect("score column").ID,
        expression_dependency::model::VirtualColFTSScoreID
    );
    assert_eq!(
        find_top_n(plan.as_ref()).expect("FTS TopN").ByItems[0]
            .Expr
            .as_column()
            .expect("TopN score column")
            .ID,
        expression_dependency::model::VirtualColFTSScoreID
    );
    assert_eq!(
        find_projection(plan.as_ref())
            .expect("FTS projection")
            .Exprs[0]
            .as_column()
            .expect("projected score column")
            .ID,
        expression_dependency::model::VirtualColFTSScoreID
    );
}

#[test]
/// 收集谓词列应填充 DataSource 的 interesting columns。
fn collect_predicate_columns_populates_data_source_interesting_columns() {
    let sql = "select a from t where b > 1 order by c";
    let (_, original) = logical_optimize_for_test(sql, 0).expect("uncollected predicate plan");
    assert!(
        find_source(original.as_ref())
            .expect("source before collection")
            .InterestingColumns
            .is_empty()
    );

    let (_, collected) =
        logical_optimize_for_test(sql, rule_dependency::FLAG_COLLECT_PREDICATE_COLUMNS_POINT)
            .expect("predicate-column collection");
    let names = find_source(collected.as_ref())
        .expect("source after collection")
        .InterestingColumns
        .iter()
        .map(|column| column.OrigName.as_str())
        .collect::<Vec<_>>();
    assert!(names.iter().any(|name| name.ends_with(".b")), "{names:?}");
    assert!(names.iter().any(|name| name.ends_with(".c")), "{names:?}");
}

#[test]
/// 关闭跨 Join/Union 聚合下推时，仍须吸收下方投影。
fn push_down_agg_substitutes_projection_expressions_when_pushdown_disabled() {
    let sql = "select sum(x) from (select a + 1 as x from t) d";
    let (_, original) = logical_optimize_for_test(sql, 0).expect("aggregate projection plan");
    let original_agg = find_aggregation(original.as_ref()).expect("original aggregation");
    assert!(
        original_agg.Children()[0]
            .as_any()
            .is::<logicalop::LogicalProjection>()
    );

    let (_, pushed) = logical_optimize_for_test(sql, rule_dependency::FLAG_PUSH_DOWN_AGG)
        .expect("aggregate push-down");
    let pushed_agg = find_aggregation(pushed.as_ref()).expect("pushed aggregation");
    assert_eq!(
        pushed_agg
            .SCtx()
            .unwrap()
            .GetSessionVars()
            .GetSystemVar(vardef_dependency::TiDBOptAggPushDown)
            .as_deref(),
        Some("OFF")
    );
    assert!(
        !pushed_agg.Children()[0]
            .as_any()
            .is::<logicalop::LogicalProjection>()
    );
    assert!(
        pushed_agg.AggFuncs[0].Args[0]
            .as_any()
            .is::<expression_dependency::ScalarFunction>()
    );
}

#[test]
/// 统计加载同步点应等待挂起项完成。
fn sync_wait_stats_load_waits_for_pending_items() {
    let handle = PlannerTestStatsHandle::success();
    let (context, optimized) = logical_optimize_with_stats_handle_for_test(
        "select a from t",
        rule_dependency::FLAG_SYNC_WAIT_STATS_LOAD_POINT,
        1,
        false,
        handle.clone(),
    )
    .expect("build pending-stats plan");
    optimized.expect("pending statistics must be completed by the installed stats handle");
    assert_eq!(handle.calls(), 1);
    assert_eq!(context.GetSessionVars().StmtCtx.PendingStatsLoadItems(), 0);
    assert!(!context.GetSessionVars().StmtCtx.IsSyncStatsFailed());
    assert!(
        context
            .GetSessionVars()
            .StmtCtx
            .StatsSyncWaitError()
            .is_none()
    );
    assert!(
        context.GetSessionVars().StmtCtx.StatsSyncWaitDuration()
            >= std::time::Duration::from_millis(1)
    );
}

#[test]
/// 统计加载同步点应处理空/失败/超时状态。
fn sync_wait_stats_load_handles_empty_failed_and_timeout_states() {
    let empty_handle = PlannerTestStatsHandle::timeout();
    let (empty_context, empty_result) = logical_optimize_with_stats_handle_for_test(
        "select a from t",
        rule_dependency::FLAG_SYNC_WAIT_STATS_LOAD_POINT,
        0,
        false,
        empty_handle.clone(),
    )
    .expect("build empty-stats plan");
    empty_result.expect("empty queue does not wait");
    assert_eq!(empty_handle.calls(), 0);
    assert_eq!(
        empty_context
            .GetSessionVars()
            .StmtCtx
            .PendingStatsLoadItems(),
        0
    );

    let failed_handle = PlannerTestStatsHandle::timeout();
    let (failed_context, failed_result) = logical_optimize_with_stats_handle_for_test(
        "select a from t",
        rule_dependency::FLAG_SYNC_WAIT_STATS_LOAD_POINT,
        1,
        true,
        failed_handle.clone(),
    )
    .expect("build previously-failed stats plan");
    failed_result.expect("previous failure is not waited twice");
    assert_eq!(failed_handle.calls(), 0);
    assert!(failed_context.GetSessionVars().StmtCtx.IsSyncStatsFailed());
    assert_eq!(
        failed_context
            .GetSessionVars()
            .StmtCtx
            .PendingStatsLoadItems(),
        1
    );

    let timeout_handle = PlannerTestStatsHandle::timeout();
    let (timeout_context, timeout_result) = logical_optimize_with_stats_handle_for_test(
        "select a from t",
        rule_dependency::FLAG_SYNC_WAIT_STATS_LOAD_POINT,
        1,
        false,
        timeout_handle.clone(),
    )
    .expect("build timeout stats plan");
    timeout_result.expect("default pseudo-timeout mode falls back without aborting optimization");
    assert_eq!(timeout_handle.calls(), 1);
    assert!(timeout_context.GetSessionVars().StmtCtx.IsSyncStatsFailed());
    assert!(
        timeout_context
            .GetSessionVars()
            .StmtCtx
            .StatsSyncWaitDuration()
            >= std::time::Duration::from_millis(5)
    );
    assert_eq!(
        timeout_context
            .GetSessionVars()
            .StmtCtx
            .StatsSyncWaitError()
            .as_deref(),
        Some("synchronous statistics wait timed out")
    );
    assert_eq!(
        timeout_context.GetSessionVars().StmtCtx.GetWarnings().len(),
        1,
        "pseudo fallback records the timeout as a statement warning"
    );
}

#[test]
/// ResolveExpand 应为 ROLLUP 层级生成投影。
fn resolve_expand_generates_rollup_level_projections() {
    let (context, child) = build_logical_for_test("select a from t").expect("rollup child");
    let column = child.Schema().Columns[0].clone();
    let active = logicalop::GroupingSet {
        ColumnIDs: std::collections::BTreeSet::from([column.UniqueID]),
    };
    let mut expand = logicalop::LogicalExpand {
        DistinctGroupByCol: vec![column.clone()],
        DistinctGbyExprs: vec![Box::new(column)],
        RollupGroupingSets: logicalop::GroupingSets(vec![
            active,
            logicalop::GroupingSet::default(),
        ]),
        ..logicalop::LogicalExpand::default()
    }
    .Init(context, 0);
    expand.SetSchema(child.Schema().Clone());
    expand.SetOutputNames(child.OutputNames().Shallow());
    expand.SetChildren(vec![child]);
    let mut resolved: logicalop::LogicalPlanRef = Box::new(expand);
    assert!(
        find_expand(resolved.as_ref())
            .expect("rollup expand")
            .LevelExprs
            .is_empty()
    );

    crate::LogicalOptimizeForTest(rule_dependency::FLAG_RESOLVE_EXPAND, &mut resolved)
        .expect("resolved rollup plan");
    let expand = find_expand(resolved.as_ref()).expect("resolved rollup expand");
    assert_eq!(expand.LevelExprs.len(), expand.RollupGroupingSets.0.len());
    assert!(!expand.LevelExprs.is_empty());
}

/// 查找带 SET 变量副作用的 Projection。
fn find_setvar_projection(
    plan: &dyn logicalop::LogicalPlan,
) -> Option<&logicalop::LogicalProjection> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .filter(|projection| {
            projection
                .Exprs
                .iter()
                .any(|expression| expression_dependency::ExprHasSetVarOrSleep(expression.as_ref()))
        })
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| find_setvar_projection(child.as_ref()))
        })
}

#[test]
/// 逻辑优化入口应跑通生产规则流水线。
fn logical_optimizer_entry_runs_production_rules() {
    let (_, plan) = logical_optimize_for_test(
        "select t.a from t join t t2 where t.a = t2.a",
        rule_dependency::FLAG_PREDICATE_PUSH_DOWN | rule_dependency::FLAG_PRUNE_COLUMNS,
    )
    .expect("logical optimizer entry must run");
    let join = find_join(plan.as_ref()).expect("optimized tree retains join");
    assert_eq!(join.EqualConditions.len(), 1);

    let (_, plan) = logical_optimize_for_test(
        "select a from (select 1 + 2 as a from t) d where d.a = 5",
        rule_dependency::FLAG_PREDICATE_PUSH_DOWN,
    )
    .expect("constant predicate must optimize");
    assert!(contains_dual(plan.as_ref()));
}

#[test]
/// 逻辑优化应保持算子结构并正确替换嵌套根。
fn logical_optimizer_preserves_operator_structure_and_replaces_nested_roots() {
    let ppd = rule_dependency::FLAG_PREDICATE_PUSH_DOWN;
    let prune = rule_dependency::FLAG_PRUNE_COLUMNS | rule_dependency::FLAG_PRUNE_COLUMNS_AGAIN;

    let (_, plan) = logical_optimize_for_test("select a as x, b from t where a > 1", ppd)
        .expect("data-source predicate plan");
    let source = find_source(plan.as_ref()).expect("predicate reaches data source");
    assert_eq!(source.AllConds.len(), 1);
    assert_eq!(source.PushedDownConds.len(), 1);
    assert_eq!(
        source.AllConds[0].HashCode(),
        source.PushedDownConds[0].HashCode()
    );
    let projection = find_projection(plan.as_ref()).expect("select projection");
    assert_eq!(projection.Exprs.len(), projection.Schema().Len());
    assert_eq!(projection.Schema().Len(), 2);
    assert_eq!(
        projection.OutputNames().0[0]
            .as_ref()
            .expect("alias name")
            .ColName
            .O,
        "x"
    );

    let (_, plan) = logical_optimize_for_test(
        "select t.a from t join t t2 on t.a = t2.a and t.b > t2.b",
        ppd | prune,
    )
    .expect("join condition plan");
    let join = find_join(plan.as_ref()).expect("join remains");
    assert_eq!(join.JoinType, logicalop::JoinType::InnerJoin);
    assert_eq!(join.EqualConditions.len(), 1);
    assert_eq!(join.OtherConditions.len(), 1);
    assert!(join.LeftConditions.is_empty());
    assert!(join.RightConditions.is_empty());

    let (_, plan) =
        logical_optimize_for_test("select a, sum(b) as s from t group by a + 1", ppd | prune)
            .expect("aggregation plan");
    let aggregation = find_aggregation(plan.as_ref()).expect("aggregation remains");
    assert_eq!(aggregation.GroupByItems.len(), 1);
    assert!(
        aggregation
            .AggFuncs
            .iter()
            .any(|function| function.Name.eq_ignore_ascii_case("sum"))
    );
    let first_row = aggregation
        .AggFuncs
        .iter()
        .find(|function| function.Name.eq_ignore_ascii_case("firstrow"))
        .expect("selected non-aggregate column is retained by firstrow");
    assert_eq!(first_row.Args.len(), 1);
    assert_eq!(
        first_row.Args[0]
            .as_column()
            .expect("firstrow source column")
            .OrigName,
        "test.t.a"
    );

    let (_, plan) = logical_optimize_for_test(
        "select (select count(*) from t where t.a = k.a) from t k",
        ppd | rule_dependency::FLAG_DECORRELATE | prune,
    )
    .expect("correlated apply plan");
    let apply = find_apply(plan.as_ref()).expect("scalar subquery retains apply");
    assert!(
        !apply.CorCols.is_empty()
            || !apply.LogicalJoin.EqualConditions.is_empty()
            || !apply.LogicalJoin.OtherConditions.is_empty()
    );
    let outer_schema = apply.Children()[0].Schema();
    let inner_schema = apply.Children()[1].Schema();
    let inner_keys = apply
        .LogicalJoin
        .EqualConditions
        .iter()
        .chain(&apply.LogicalJoin.OtherConditions)
        .flat_map(|condition| expression_dependency::ExtractColumns(condition.as_ref()))
        .filter(|column| !outer_schema.Contains(column))
        .collect::<Vec<_>>();
    if inner_keys.is_empty() {
        assert!(
            !apply.CorCols.is_empty(),
            "an unlifted equality remains represented by Apply correlation"
        );
    } else {
        for inner_key in &inner_keys {
            assert!(
                inner_schema.Contains(inner_key),
                "lifted inner column {} must be reachable from the Apply inner schema",
                inner_key.UniqueID
            );
        }
    }
    let projection = find_projection(apply.Children()[1].as_ref())
        .expect("scalar aggregation inner path retains its Projection");
    assert_eq!(projection.Exprs.len(), projection.Schema().Len());
    for inner_key in inner_keys {
        let offset = projection
            .Schema()
            .ColumnIndex(inner_key)
            .expect("inner Projection exposes every lifted inner key");
        assert_eq!(
            projection.Exprs[offset]
                .as_column()
                .expect("lifted Projection output is backed by its inner column")
                .UniqueID,
            inner_key.UniqueID
        );
    }

    let (_, plan) = logical_optimize_for_test(
        "select * from (select a from t union all select a from t union all select a from t) u where a > 1",
        ppd,
    )
    .expect("union predicate plan");
    let union = find_union(plan.as_ref()).expect("flattened union");
    assert_eq!(union.Children().len(), 3);
    for branch in union.Children() {
        let source = find_source(branch.as_ref()).expect("union branch source");
        assert_eq!(source.AllConds.len(), 1);
        assert_eq!(source.PushedDownConds.len(), 1);
    }

    let (_, plan) = logical_optimize_for_test("select sum(a) over() from t where 1 = 0", ppd)
        .expect("window over empty source");
    assert_eq!(count_type::<logicalop::LogicalWindow>(plan.as_ref()), 1);
    assert!(contains_dual(plan.as_ref()));
    assert_eq!(count_type::<logicalop::DataSource>(plan.as_ref()), 0);

    let (_, plan) = logical_optimize_for_test(
        "select * from t t1 join t t2 on t1.a = t2.a where t2.a = null",
        ppd,
    )
    .expect("null-rejected root join");
    assert!(contains_dual(plan.as_ref()));
    assert_eq!(count_type::<logicalop::LogicalJoin>(plan.as_ref()), 0);
}

#[test]
/// 规则应按 Go 顺序派发，且 flag 位彼此隔离。
fn logical_optimizer_dispatches_rules_in_go_order_and_isolates_flags() {
    let (_, without_ppd) = logical_optimize_for_test("select a from t where a > 1", 0)
        .expect("unoptimized logical plan");
    assert!(
        find_source(without_ppd.as_ref())
            .expect("source without ppd")
            .AllConds
            .is_empty()
    );
    assert_eq!(
        count_type::<logicalop::LogicalSelection>(without_ppd.as_ref()),
        1
    );

    let (_, with_ppd) = logical_optimize_for_test(
        "select a from t where a > 1",
        rule_dependency::FLAG_PREDICATE_PUSH_DOWN,
    )
    .expect("ppd logical plan");
    assert_eq!(
        find_source(with_ppd.as_ref())
            .expect("source with ppd")
            .AllConds
            .len(),
        1
    );
    assert_eq!(
        count_type::<logicalop::LogicalSelection>(with_ppd.as_ref()),
        0
    );

    let sql = "select t.a from t left join t t2 on t.a = t2.a where t2.b > 1";
    let (_, without_convert) = logical_optimize_for_test(sql, 0).expect("outer join plan");
    assert_eq!(
        find_join(without_convert.as_ref())
            .expect("outer join without convert flag")
            .JoinType,
        logicalop::JoinType::LeftOuterJoin
    );
    let (_, with_convert) =
        logical_optimize_for_test(sql, rule_dependency::FLAG_CONVERT_OUTER_TO_INNER_JOIN)
            .expect("converted join plan");
    assert_eq!(
        find_join(with_convert.as_ref())
            .expect("join with convert flag")
            .JoinType,
        logicalop::JoinType::InnerJoin
    );

    super::optimizer_runtime::StartLogicalRuleTrace();
    let (_, ordered) = logical_optimize_for_test(
        "select a from t where a > 1",
        rule_dependency::FLAG_PRUNE_COLUMNS
            | rule_dependency::FLAG_PREDICATE_PUSH_DOWN
            | rule_dependency::FLAG_PREDICATE_SIMPLIFICATION
            | rule_dependency::FLAG_PRUNE_COLUMNS_AGAIN,
    )
    .expect("Go-ordered rule subset");
    let source = find_source(ordered.as_ref()).expect("ordered source");
    assert_eq!(source.Schema().Len(), 1);
    assert_eq!(source.AllConds.len(), 1);

    assert_eq!(
        super::optimizer_runtime::TakeLogicalRuleTrace(),
        vec![
            super::optimizer_runtime::LogicalRule::PruneColumns,
            super::optimizer_runtime::LogicalRule::PredicatePushDown,
            super::optimizer_runtime::LogicalRule::PredicateSimplification,
            super::optimizer_runtime::LogicalRule::PruneColumnsAgain,
        ],
        "dispatcher must execute enabled rules in Go optRuleList order"
    );
}

#[test]
/// 谓词下推在语义屏障处须保留相关列约束。
fn predicate_push_down_keeps_correlation_with_semantic_barriers() {
    fn assert_correlated_apply(plan: &dyn logicalop::LogicalPlan, operator: &str) {
        let apply = find_apply(plan).unwrap_or_else(|| panic!("{operator} must retain Apply"));
        assert!(
            !apply.CorCols.is_empty(),
            "{operator} Apply must retain its correlated-column binding"
        );
    }

    let ppd = rule_dependency::FLAG_PREDICATE_PUSH_DOWN;

    let (_, limited) = logical_optimize_for_test(
        "select (select t2.a from t t2 where t2.b = t1.b limit 1) from t t1",
        ppd,
    )
    .expect("correlated LIMIT plan");
    assert!(find_limit(limited.as_ref()).is_some(), "LIMIT remains");
    assert_correlated_apply(limited.as_ref(), "Limit");

    let (_, scalar) = logical_optimize_for_test(
        "select (select t2.a from t t2 where t2.b = t1.b) from t t1",
        ppd,
    )
    .expect("correlated scalar-subquery plan");
    assert!(
        find_max_one_row(scalar.as_ref()).is_some(),
        "MaxOneRow remains"
    );
    assert_correlated_apply(scalar.as_ref(), "MaxOneRow");

    let (_, setvar) = logical_optimize_for_test(
        "select (select setvar('x', t2.a + 1) from t t2 where t2.b = t1.b) from t t1",
        ppd,
    )
    .expect("correlated setvar projection plan");
    assert!(
        find_setvar_projection(setvar.as_ref()).is_some(),
        "setvar Projection remains"
    );
    assert_correlated_apply(setvar.as_ref(), "setvar Projection");

    let combined = ppd | rule_dependency::FLAG_DECORRELATE;
    let (_, limited) = logical_optimize_for_test(
        "select (select t2.a from t t2 where t2.b = t1.b limit 1) from t t1",
        combined,
    )
    .expect("decorrelation must respect LIMIT");
    assert!(find_limit(limited.as_ref()).is_some(), "LIMIT remains");
    assert_correlated_apply(limited.as_ref(), "Limit");

    let (_, scalar) = logical_optimize_for_test(
        "select (select t2.a from t t2 where t2.b = t1.b) from t t1",
        combined,
    )
    .expect("decorrelation must respect MaxOneRow");
    assert!(
        find_max_one_row(scalar.as_ref()).is_some(),
        "MaxOneRow remains"
    );
    assert_correlated_apply(scalar.as_ref(), "MaxOneRow");

    let (_, setvar) = logical_optimize_for_test(
        "select (select setvar('x', t2.a + 1) from t t2 where t2.b = t1.b) from t t1",
        combined,
    )
    .expect("decorrelation must respect a setvar Projection");
    assert!(
        find_setvar_projection(setvar.as_ref()).is_some(),
        "setvar Projection remains"
    );
    assert_correlated_apply(setvar.as_ref(), "setvar Projection");
}
