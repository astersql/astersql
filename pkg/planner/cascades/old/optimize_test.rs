// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Cascades `Optimizer` 私有阶段方法的单元测试。
//
// 覆盖 Convert2Group 的 schema 初始化、implGroup 代价剪枝、
// fillGroupStats 填充统计，以及 exploration 每表达式只应用一次规则。

// 本文件对应 pkg/planner/cascades/old/optimize_test.go。Go 版本每个测试都通过
// "SQL -> parser -> BuildLogicalPlanForTest" 拿到 LogicalPlan，这条链路依赖的 SQL
// 解析/优化管线不在本任务 writes 范围内、也不存在于当前 Rust 窄运行时；这里改为直接手工
// 构造最小 LogicalPlan（LogicalTableDual 叶子节点），驱动与 Go 完全相同的 Optimizer 私有
// 阶段方法（implGroup/fillGroupStats/onPhaseExploration）来验证同样的行为。
//
// 本文件以 `#[path = "optimize_test.rs"]` 挂在 optimize.rs 内部（而不是 lib.rs 的兄弟模块），
// 因为需要访问 Optimizer 的私有阶段方法，Rust 的模块可见性只允许父模块的后代访问私有 item。
//
// 覆盖：
// - TestInitGroupSchema：Convert2Group 只初始化 schema，不填 stats。
// - TestImplGroupZeroCost：implGroup 在 costLimit 为 0 时找不到可行物理实现。
// - TestFillGroupStats：fillGroupStats 会为叶子 group 填充统计信息。
// - TestAppliedRuleSet：exploration 在一轮内只会把同一个 group 表达式交给规则处理一次。

use super::*;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use astersql_expression as expression;
use astersql_expression_exprstatic as exprstatic;
use astersql_planner_planctx as planctx;
use logicalop::{LogicalAggregation, LogicalIndexScan, LogicalJoin, LogicalTableDual};

/// 测试用 PlanContext，与 optimize 私有阶段对接。
struct TestPlanContext {
    plan_id: AtomicI32,
    session: planctx::variable::SessionVars,
    expression: Arc<exprstatic::ExprContext>,
    build_pb: astersql_planner_core_base::BuildPBContext,
    builtin_function_usage: astersql_planner_core_base::BuiltinFunctionUsageCounter,
}

impl TestPlanContext {
    fn new() -> Self {
        let expression = Arc::new(exprstatic::NewExprContext(Vec::new()));
        let expression_for_build: Arc<dyn planctx::exprctx::BuildContext> = expression.clone();
        Self {
            plan_id: AtomicI32::new(0),
            session: planctx::variable::SessionVars::default(),
            expression,
            build_pb: astersql_planner_core_base::BuildPBContext {
                ExprCtx: expression_for_build,
                Client: None,
                TiFlashFastScan: false,
                TiFlashFineGrainedShuffleBatchSize: 0,
                GroupConcatMaxLen: 0,
                InExplainStmt: false,
                WarnHandler: None,
                ExtraWarnghandler: None,
            },
            builtin_function_usage:
                astersql_planner_core_base::BuiltinFunctionUsageCounter::default(),
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
        panic!("optimize tests do not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }

    fn GetBuildPBCtx(&self) -> &astersql_planner_core_base::BuildPBContext {
        &self.build_pb
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

/// 构造测试 PlanContext。
fn context() -> astersql_planner_core_base::ContextRef {
    Arc::new(TestPlanContext::new())
}

/// 单列 Schema 工厂。
fn one_column_schema() -> expression::Schema {
    expression::NewSchema(vec![expression::Column::default()])
}

fn column(unique_id: i64) -> expression::Column {
    expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        unique_id,
        unique_id,
        0,
    )
}

/// 构造带固定行数的 LogicalTableDual 叶子计划。
fn leaf_table_dual(ctx: astersql_planner_core_base::ContextRef, row_count: i32) -> LogicalPlanRef {
    let mut dual = LogicalTableDual::default().Init(ctx, 0);
    dual.RowCount = row_count;
    dual.LogicalSchemaProducer.SetSchema(one_column_schema());
    Box::new(dual)
}

/// 构造与 Go 用例同类的二元 Join，让统计推导和物理实现都递归经过孩子 Group。
fn join_of_duals(ctx: astersql_planner_core_base::ContextRef) -> LogicalPlanRef {
    let mut join = LogicalJoin::default().Init(ctx.clone(), 0);
    join.LogicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![column(1), column(2)]));
    join.SetChildren(vec![
        leaf_table_dual(ctx.clone(), 2),
        leaf_table_dual(ctx, 3),
    ]);
    Box::new(join)
}

// test_init_group_schema 对应 Go 的 TestInitGroupSchema：Convert2Group 会初始化 group property
// 和一列 schema，但不会填充 stats（stats 是 fillGroupStats 的职责，属于 implementation 阶段）。
#[test]
fn test_init_group_schema() {
    let ctx = context();
    let group = memo::Convert2Group(leaf_table_dual(ctx, 1));
    let group = group.borrow();
    let schema = group
        .Prop
        .Schema
        .as_deref()
        .expect("Go require.NotNil(g.Prop.Schema)");
    assert_eq!(
        schema.Columns.len(),
        1,
        "Go require.Equal(1, g.Prop.Schema.Len())"
    );
    assert!(group.Prop.Stats.is_none(), "Go require.Nil(g.Prop.Stats)");
}

// test_impl_group_zero_cost 对应 Go 的 TestImplGroupZeroCost：costLimit 低于任何真实物理实现
// 的代价时，implGroup 找不到满足要求的候选，返回 None；与 Go 一样使用 Join 和 0 costLimit。
#[test]
fn test_impl_group_zero_cost() {
    let ctx = context();
    let group = memo::Convert2Group(join_of_duals(ctx));
    let mut prop = PhysicalProperty::default();
    prop.ExpectedCnt = f64::MAX;
    let implementation = Optimizer::NewOptimizer()
        .implGroup(&group, &prop, 0.0)
        .expect("Go require.NoError");
    assert!(implementation.is_none(), "Go require.Nil(impl)");
}

// test_fill_group_stats 对应 Go 的 TestFillGroupStats：fillGroupStats 会为叶子 group 填充
// 统计信息；与 Go 一样使用 Join，覆盖孩子 Group 的递归统计推导。
#[test]
fn test_fill_group_stats() {
    let ctx = context();
    let group = memo::Convert2Group(join_of_duals(ctx));
    Optimizer::NewOptimizer()
        .fillGroupStats(&group)
        .expect("Go require.NoError");
    let stats = group
        .borrow()
        .Prop
        .Stats
        .as_deref()
        .cloned()
        .expect("Go require.NotNil(rootGroup.Prop.Stats)");
    assert!(stats.RowCount > 0.0);
}

// test_prepare_possible_properties 对应 Go 的 TestPreparePossibleProperties：聚合只保留
// 能覆盖 group-by 列的索引顺序，并把结果写回 group 属性与调用方缓存。
#[test]
fn test_prepare_possible_properties() {
    let ctx = context();
    let column_f = column(1);
    let column_a = column(2);

    let mut scan = LogicalIndexScan {
        IdxCols: vec![column_f.Clone(), column_a.Clone()],
        EqCondCount: 1,
        ..LogicalIndexScan::default()
    }
    .Init(ctx.clone(), 0);
    scan.LogicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![column_f.Clone(), column_a]));

    let mut aggregation = LogicalAggregation {
        GroupByItems: vec![Box::new(column_f.Clone())],
        ..LogicalAggregation::default()
    }
    .Init(ctx, 0);
    aggregation
        .LogicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![column_f.Clone()]));
    aggregation.SetChildren(vec![Box::new(scan)]);

    let group = memo::Convert2Group(Box::new(aggregation));
    let mut property_map = HashMap::new();
    let properties = preparePossibleProperties(&group, &mut property_map);

    assert_eq!(properties.Orders.len(), 1);
    assert_eq!(properties.Orders[0].len(), 1);
    assert_eq!(properties.Orders[0][0].UniqueID, column_f.UniqueID);
    let group = group.borrow();
    assert_eq!(group.Prop.PossibleProps.len(), 1);
    assert_eq!(group.Prop.PossibleProps[0].len(), 1);
    assert_eq!(group.Prop.PossibleProps[0][0].UniqueID, column_f.UniqueID);
    assert!(property_map.contains_key(&group.ID()));
}

// FakeTransformation 对应 Go 测试里的 fakeTransformation：记录被真正调用 OnTransform 的次数，
// 但不改写 memo（eraseOld=false, eraseAll=false），这样断言只关心"探索一轮时，每个 group
// 表达式是否恰好被规则处理一次"。
/// 仅计数 OnTransform 调用次数、不改写 memo 的假变换规则。
struct FakeTransformation {
    pattern: pattern::Pattern,
    applied_times: Rc<Cell<i32>>,
}

impl crate::Transformation for FakeTransformation {
    fn get_pattern(&self) -> &pattern::Pattern {
        &self.pattern
    }
    fn matches(&self, _expr: &memo::ExprIter) -> bool {
        true
    }
    fn on_transform(&self, _old: &memo::ExprIter) -> crate::TransformResult {
        self.applied_times.set(self.applied_times.get() + 1);
        Ok((Vec::new(), false, false))
    }
}

// test_applied_rule_set 对应 Go 的 TestAppliedRuleSet：同一个规则在一轮 exploration 里只会
// 被真正应用一次（每个 group 表达式在本轮只探索一次），而不是每次 findMoreEquiv 扫描都重复计数。
#[test]
fn test_applied_rule_set() {
    let ctx = context();
    let mut projection = LogicalProjection::default().Init(ctx.clone(), 0);
    projection
        .LogicalSchemaProducer
        .SetSchema(one_column_schema());
    projection.SetChildren(vec![leaf_table_dual(ctx.clone(), 1)]);

    let applied_times = Rc::new(Cell::new(0));
    let rule = FakeTransformation {
        pattern: pattern::NewPattern(pattern::OperandProjection, pattern::EngineAll),
        applied_times: applied_times.clone(),
    };
    let mut batch = TransformationRuleBatch::new();
    batch.insert(
        pattern::OperandProjection,
        vec![Box::new(rule) as Box<dyn crate::Transformation>],
    );

    let mut optimizer = Optimizer::NewOptimizer();
    optimizer.ResetTransformationRules(vec![batch]);

    let group = memo::Convert2Group(Box::new(projection));
    optimizer
        .onPhaseExploration(ctx.as_ref(), &group)
        .expect("Go require.NoError");
    assert_eq!(
        applied_times.get(),
        1,
        "Go require.Equal(1, rule.appliedTimes)"
    );
}
