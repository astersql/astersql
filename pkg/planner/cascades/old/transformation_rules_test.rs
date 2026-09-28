// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 变换规则真实 `matches`/`on_transform` 行为的单元测试。
//
// 通过手工 LogicalPlan + `NewExprIterFromGroupElem` 绑定，覆盖 Limit→TableDual
// 与相邻 Limit 合并（空/非空窗口）两条路径。

// 本文件对应 pkg/planner/cascades/old/transformation_rules_test.go。Go 版本对每条规则都通过
// SQL -> BuildLogicalPlanForTest -> onPhasePreprocessing -> onPhaseExploration 的完整管线，再用
// testdata golden 文件比对 ToString 输出；这条管线依赖的 parser/domain/infoschema SQL
// 构建能力目前不在本 crate 的 writes 范围内、也不存在于当前 Rust 窄运行时中，因此这里改为
// 直接构造最小 LogicalPlan 树（LogicalTableDual 作为叶子），驱动与 exploreGroup/findMoreEquiv
// 完全相同的入口——`memo::NewExprIterFromGroupElem` + `Transformation::matches`/`on_transform`
// ——来验证具体规则的真实转换逻辑，而不是重新实现或简化生产代码。
//
// 本文件以 `#[path = "transformation_rules_test.rs"]` 挂在 transformation_rules.rs 内部
// （而不是 lib.rs 的兄弟模块），因为需要访问该文件内部私有 `mod memo` 中定义的 `ExprIterExt`
// 帮助 trait（child()/logical_limit() 等），Rust 的模块可见性只允许父模块的后代访问私有 item。
//
// 覆盖的规则：
// - TestTransformLimitToTableDual (Go) -> RuleTransformLimitToTableDual：Count=0 的 Limit
//   被直接改写为同 schema 的空 TableDual，且 eraseAll=true。
// - TestMergeAdjacentLimit (Go) -> RuleMergeAdjacentLimit：相邻两层 Limit 求区间交集；
//   同时覆盖"区间为空 -> eraseAll 产出空 TableDual"与"区间非空 -> 合并成一层 Limit"两条分支。

use super::memo;
use super::*;
use astersql_expression as expression;
use astersql_expression_exprstatic as exprstatic;
use astersql_planner_core_base::{
    BuildPBContext, BuiltinFunctionUsageCounter, ContextRef, PlanContext,
};
use logicalop::{LogicalLimit, LogicalPlan, LogicalProjection, LogicalTableDual, LogicalTopN};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

struct SupportedPushDownClient;

impl astersql_kv::Client for SupportedPushDownClient {
    fn Send(
        &self,
        _ctx: &astersql_kv::Context,
        _request: &astersql_kv::Request,
        _variables: &dyn std::any::Any,
        _option: &astersql_kv::ClientSendOption,
    ) -> Option<Box<dyn astersql_kv::Response>> {
        panic!("pushdown classification must not send a KV request")
    }

    fn IsRequestTypeSupported(&self, _request_type: i64, _sub_type: i64) -> bool {
        true
    }
}

/// 测试用 PlanContext。
struct TestPlanContext {
    plan_id: AtomicI32,
    session: planctx::variable::SessionVars,
    expression: Arc<exprstatic::ExprContext>,
    build_pb: BuildPBContext,
    builtin_function_usage: BuiltinFunctionUsageCounter,
}

impl TestPlanContext {
    fn new() -> Self {
        let expression = Arc::new(exprstatic::NewExprContext(Vec::new()));
        let expression_for_build: Arc<dyn planctx::exprctx::BuildContext> = expression.clone();
        Self {
            plan_id: AtomicI32::new(0),
            session: planctx::variable::SessionVars::default(),
            expression,
            build_pb: BuildPBContext {
                ExprCtx: expression_for_build,
                Client: Some(Arc::new(SupportedPushDownClient)),
                TiFlashFastScan: false,
                TiFlashFineGrainedShuffleBatchSize: 0,
                GroupConcatMaxLen: 0,
                InExplainStmt: false,
                WarnHandler: None,
                ExtraWarnghandler: None,
            },
            builtin_function_usage: BuiltinFunctionUsageCounter::default(),
        }
    }
}

impl PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.session
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("transformation rule tests do not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }

    fn GetBuildPBCtx(&self) -> &BuildPBContext {
        &self.build_pb
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

/// 构造测试 PlanContext。
fn context() -> ContextRef {
    Arc::new(TestPlanContext::new())
}

/// 单列 Schema 工厂。
fn one_column_schema() -> expression::Schema {
    expression::NewSchema(vec![expression::Column::default()])
}

/// 构造指定 schema 与行数的 TableDual。
fn table_dual(
    ctx: ContextRef,
    schema: &expression::Schema,
    row_count: i32,
) -> Box<dyn LogicalPlan> {
    let mut dual = LogicalTableDual::default().Init(ctx, 0);
    dual.RowCount = row_count;
    dual.LogicalSchemaProducer.SetSchema(schema.Clone());
    Box::new(dual)
}

/// 构造指定 offset/count 的 LogicalLimit。
fn limit(ctx: ContextRef, schema: &expression::Schema, offset: u64, count: u64) -> LogicalLimit {
    let mut node = LogicalLimit::default().Init(ctx, 0);
    node.Offset = offset;
    node.Count = count;
    node.LogicalSchemaProducer.SetSchema(schema.Clone());
    node
}

#[test]
fn test_push_down_exprs_classifies_pb_encodable_constant_like_go() {
    let ctx = context();
    let condition: expression::ExprBox = Box::new(expression::Constant::with_type(
        expression::types::NewIntDatum(1),
        *expression::types::NewFieldType(mysql::TypeLonglong),
    ));
    let (pushed, remained) = super::expression::PushDownExprs(
        util::GetPushDownCtx(ctx.as_ref()),
        vec![condition],
        kv::TiKV,
    );
    assert_eq!(pushed.len(), 1);
    assert!(remained.is_empty());
}

#[test]
fn test_propagate_constant_uses_expression_solver_with_borrowed_context() {
    use expression::Expression as _;

    let ctx = context();
    let field_type = *expression::types::NewFieldType(mysql::TypeLonglong);
    let column0 = expression::Column::new(field_type.clone(), 0, 100, 0);
    let column1 = expression::Column::new(field_type.clone(), 1, 101, 1);
    let one = || {
        Box::new(expression::Constant::with_type(
            expression::types::NewIntDatum(1),
            field_type.clone(),
        )) as expression::ExprBox
    };
    let eq = |left: expression::ExprBox, right: expression::ExprBox| {
        expression::NewFunctionInternal(
            ctx.GetExprCtx(),
            ast::EQ,
            field_type.clone(),
            vec![left, right],
        )
        .expect("eq expression is valid")
    };
    let propagated = super::expression::PropagateConstant(
        ctx.GetExprCtx(),
        vec![
            eq(Box::new(column0.Clone()), Box::new(column1.Clone())),
            eq(Box::new(column1), one()),
        ],
    );
    let mut rendered = propagated
        .iter()
        .map(|condition| {
            condition.StringWithCtx(
                Some(ctx.GetExprCtx().GetEvalCtx()),
                expression::errors::RedactLogDisable,
            )
        })
        .collect::<Vec<_>>();
    rendered.sort();
    assert_eq!(rendered, vec!["eq(Column#100, 1)", "eq(Column#101, 1)"]);
}

// test_transform_limit_to_table_dual_rewrites_zero_count_limit 对应 Go 的
// TestTransformLimitToTableDual：Count=0 的 Limit 无论子节点是什么，都会被直接改写成同 schema
// 的空 TableDual，且 eraseAll=true（因为 RuleTransformLimitToTableDual 的 Pattern 不约束子节点）。
#[test]
fn test_transform_limit_to_table_dual_rewrites_zero_count_limit() {
    let ctx = context();
    let schema = one_column_schema();

    let child = table_dual(ctx.clone(), &schema, 1);
    let mut zero_limit = limit(ctx.clone(), &schema, 0, 0);
    zero_limit.SetChildren(vec![child]);

    let group = memo::Convert2Group(Box::new(zero_limit));
    let rule = super::NewRuleTransformLimitToTableDual();
    let iter = memo::NewExprIterFromGroupElem(&group, 0, rule.get_pattern())
        .expect("a lone Limit expression matches a childless-pattern rule");
    assert!(
        rule.matches(&iter),
        "Go: matches returns true when limit.Count == 0"
    );

    let (new_exprs, erase_old, erase_all) = rule
        .on_transform(&iter)
        .expect("Go require.NoError from OnTransform");
    assert!(erase_old, "Go OnTransform returns eraseOld=true");
    assert!(erase_all, "Go OnTransform returns eraseAll=true");
    assert_eq!(new_exprs.len(), 1);

    let rewritten = new_exprs[0].borrow();
    let dual = rewritten
        .ExprNode
        .as_any()
        .downcast_ref::<LogicalTableDual>()
        .expect("Go: the rewritten node is a *LogicalTableDual");
    assert_eq!(dual.RowCount, 0, "Go: dual.RowCount is always 0 here");
    assert_eq!(dual.Schema().Columns.len(), schema.Columns.len());
    assert!(rewritten.Children.is_empty());
}

// test_merge_adjacent_limit_intersects_the_two_windows 对应 Go 的
// TestMergeAdjacentLimit（非空交集分支）：新 offset = 内层 offset + 外层 offset，
// 新 count = min(内层 count - 外层 offset, 外层 count)，且新 Limit 直接接管内层 Limit 原来的
// 子 Group（内层 Limit 本身从结果树中消失）。
#[test]
fn test_merge_adjacent_limit_intersects_the_two_windows() {
    let ctx = context();
    let schema = one_column_schema();

    let leaf = table_dual(ctx.clone(), &schema, 1);
    let mut inner = limit(ctx.clone(), &schema, 1, 10);
    inner.SetChildren(vec![leaf]);
    let mut outer = limit(ctx.clone(), &schema, 2, 5);
    outer.SetChildren(vec![Box::new(inner)]);

    let group = memo::Convert2Group(Box::new(outer));
    let rule = super::NewRuleMergeAdjacentLimit();
    let iter = memo::NewExprIterFromGroupElem(&group, 0, rule.get_pattern())
        .expect("outer Limit over inner Limit matches the unary Limit/Limit pattern");
    assert!(rule.matches(&iter));

    let (new_exprs, erase_old, erase_all) = rule
        .on_transform(&iter)
        .expect("Go require.NoError from OnTransform");
    assert!(erase_old);
    assert!(
        !erase_all,
        "Go: the intersecting window keeps eraseAll=false"
    );
    assert_eq!(new_exprs.len(), 1);

    let merged = new_exprs[0].borrow();
    let merged_limit = merged
        .ExprNode
        .as_any()
        .downcast_ref::<LogicalLimit>()
        .expect("Go: the merged node is a *LogicalLimit");
    assert_eq!(
        merged_limit.Offset, 3,
        "1 (inner offset) + 2 (outer offset)"
    );
    assert_eq!(merged_limit.Count, 5, "min(10 - 2, 5)");
    // The merged Limit reattaches directly to the inner Limit's child group,
    // so the inner Limit's own group is no longer part of the resulting tree.
    assert_eq!(merged.Children.len(), 1);
}

// Go 的 uint64 加法在 offset 溢出时按模 2^64 回绕。Rust debug 构建也必须保持
// 这个边界语义，不能因直接使用 `+` 而 panic。
#[test]
fn test_merge_adjacent_limit_wraps_offset_like_go_uint64() {
    let ctx = context();
    let schema = one_column_schema();

    let leaf = table_dual(ctx.clone(), &schema, 1);
    let mut inner = limit(ctx.clone(), &schema, u64::MAX, 2);
    inner.SetChildren(vec![leaf]);
    let mut outer = limit(ctx, &schema, 1, 1);
    outer.SetChildren(vec![Box::new(inner)]);

    let group = memo::Convert2Group(Box::new(outer));
    let rule = super::NewRuleMergeAdjacentLimit();
    let iter = memo::NewExprIterFromGroupElem(&group, 0, rule.get_pattern())
        .expect("outer Limit over inner Limit matches the unary Limit/Limit pattern");

    let (new_exprs, erase_old, erase_all) = rule
        .on_transform(&iter)
        .expect("Go uint64 offset addition wraps instead of panicking");
    assert!(erase_old);
    assert!(!erase_all);
    let merged = new_exprs[0].borrow();
    let merged_limit = merged
        .ExprNode
        .as_any()
        .downcast_ref::<LogicalLimit>()
        .expect("the merged node is a LogicalLimit");
    assert_eq!(merged_limit.Offset, 0);
    assert_eq!(merged_limit.Count, 1);
}

// Go 当前实现会把 ColumnSubstitute 后的常量排序项保留在新 TopN 中；被擦除的是旧
// TopN 上的排序项。Rust 必须复现最终 memo 候选的这一可观察结果。
#[test]
fn test_push_top_n_down_projection_keeps_constant_sort_item_like_go() {
    let ctx = context();
    let schema = one_column_schema();

    let leaf = table_dual(ctx.clone(), &schema, 1);
    let mut projection = LogicalProjection::default().Init(ctx.clone(), 0);
    projection.Exprs = vec![Box::new(expression::Column::default())];
    projection.LogicalSchemaProducer.SetSchema(schema.Clone());
    projection.SetChildren(vec![leaf]);

    let mut top_n = LogicalTopN::default().Init(ctx, 0);
    top_n.ByItems = vec![util::ByItems {
        Expr: Box::new(expression::Constant::null(mysql::TypeNull)),
        Desc: false,
    }];
    top_n.Count = 1;
    top_n.LogicalSchemaProducer.SetSchema(schema);
    top_n.SetChildren(vec![Box::new(projection)]);

    let group = memo::Convert2Group(Box::new(top_n));
    let rule = super::NewRulePushTopNDownProjection();
    let iter = memo::NewExprIterFromGroupElem(&group, 0, rule.get_pattern())
        .expect("TopN over Projection matches the push-down rule");
    assert!(rule.matches(&iter));

    let (new_exprs, erase_old, erase_all) = rule
        .on_transform(&iter)
        .expect("Go push-down transformation succeeds");
    assert!(erase_old);
    assert!(!erase_all);

    let rewritten_projection = new_exprs[0].borrow();
    let top_n_group = rewritten_projection.Children[0].borrow();
    let pushed_top_n_expr = top_n_group.Equivalents[0].borrow();
    let pushed_top_n = pushed_top_n_expr
        .ExprNode
        .as_any()
        .downcast_ref::<LogicalTopN>()
        .expect("the Projection child is the pushed-down TopN");
    assert_eq!(pushed_top_n.ByItems.len(), 1);
    assert!(
        pushed_top_n.ByItems[0]
            .Expr
            .as_any()
            .is::<expression::Constant>()
    );
}

// test_merge_adjacent_limit_erases_everything_on_empty_window 对应 Go 的
// TestMergeAdjacentLimit（空交集分支）：当内层 count <= 外层 offset 时结果集必然为空，
// RuleMergeAdjacentLimit 直接把整棵子树替换成同 schema 的空 TableDual，并要求 eraseAll=true。
#[test]
fn test_merge_adjacent_limit_erases_everything_on_empty_window() {
    let ctx = context();
    let schema = one_column_schema();

    let leaf = table_dual(ctx.clone(), &schema, 1);
    let mut inner = limit(ctx.clone(), &schema, 0, 2);
    inner.SetChildren(vec![leaf]);
    let mut outer = limit(ctx.clone(), &schema, 5, 3);
    outer.SetChildren(vec![Box::new(inner)]);

    let group = memo::Convert2Group(Box::new(outer));
    let rule = super::NewRuleMergeAdjacentLimit();
    let iter = memo::NewExprIterFromGroupElem(&group, 0, rule.get_pattern())
        .expect("outer Limit over inner Limit matches the unary Limit/Limit pattern");
    assert!(rule.matches(&iter));

    let (new_exprs, erase_old, erase_all) = rule
        .on_transform(&iter)
        .expect("Go require.NoError from OnTransform");
    assert!(erase_old);
    assert!(erase_all, "Go: an empty window forces eraseAll=true");
    assert_eq!(new_exprs.len(), 1);

    let rewritten = new_exprs[0].borrow();
    let dual = rewritten
        .ExprNode
        .as_any()
        .downcast_ref::<LogicalTableDual>()
        .expect("Go: the rewritten node is a *LogicalTableDual");
    assert_eq!(dual.RowCount, 0);
}
