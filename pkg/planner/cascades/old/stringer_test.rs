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

// `ToString` 与 exploration 改写可观测性的测试。
//
// 手工构造最小 LogicalPlan，验证未探索与规则改写后的 memo 字符串输出。

// 本文件对应 pkg/planner/cascades/old/stringer_test.go 的 TestGroupStringer。Go 版本从 SQL
// 解析出 LogicalPlan 后跑 onPhasePreprocessing + onPhaseExploration，再用 testdata golden 文件
// 比对 ToString 输出；这条 SQL 构建链路不在本任务 writes 范围、也不存在于当前 Rust 窄运行时，
// 因此改为手工构造最小 LogicalPlan 树，分别验证：
// 1) 未经过 exploration 时 ToString 对 Group 树结构（分组编号、Schema、子节点引用）的真实渲染；
// 2) 接入真实 Optimizer 私有阶段（onPhasePreprocessing + onPhaseExploration）后，
//    memo 结构真的按 transformation rule 被改写，ToString 能观察到这一变化。
//
// 本文件以 `#[path = "stringer_test.rs"]` 挂在 optimize.rs 内部（而不是 lib.rs 的兄弟模块），
// 因为第二个测试需要访问 Optimizer 的私有阶段方法。

use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use astersql_expression as expression;
use astersql_expression_exprstatic as exprstatic;
use astersql_planner_planctx as planctx;
use logicalop::{LogicalSelection, LogicalTableDual};

/// 测试用 PlanContext：提供表达式上下文与 plan id 分配，不构建 range。
struct TestPlanContext {
    plan_id: AtomicI32,
    ignore_explain_id_suffix: bool,
    session: planctx::variable::SessionVars,
    expression: Arc<exprstatic::ExprContext>,
    build_pb: astersql_planner_core_base::BuildPBContext,
    builtin_function_usage: astersql_planner_core_base::BuiltinFunctionUsageCounter,
}

impl TestPlanContext {
    fn new(ignore_explain_id_suffix: bool) -> Self {
        let expression = Arc::new(exprstatic::NewExprContext(Vec::new()));
        let expression_for_build: Arc<dyn planctx::exprctx::BuildContext> = expression.clone();
        Self {
            plan_id: AtomicI32::new(0),
            ignore_explain_id_suffix,
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
        self.ignore_explain_id_suffix
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.session
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("stringer tests do not build ranges")
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

/// 构造共享的测试 PlanContext 引用。
fn context() -> astersql_planner_core_base::ContextRef {
    Arc::new(TestPlanContext::new(false))
}

/// 单列默认 Schema，足够驱动 stringer 渲染。
fn one_column_schema() -> expression::Schema {
    expression::NewSchema(vec![expression::Column::default()])
}

// test_to_string_renders_group_tree_without_exploration 验证 ToString 在没有做任何
// exploration 的情况下，也能正确地：按先出现顺序给子 Group 编号、渲染每个 Group 的 schema 行、
// 以及为每个 group 表达式渲染 "TP_ID input:[Group#n], ExplainInfo" 一行。
#[test]
fn test_to_string_renders_group_tree_without_exploration() {
    let ctx = context();
    let schema = one_column_schema();

    let mut dual = LogicalTableDual::default().Init(ctx.clone(), 0);
    dual.RowCount = 1;
    dual.LogicalSchemaProducer.SetSchema(schema.Clone());
    let dual_id = dual.ID();

    let mut selection = LogicalSelection::default().Init(ctx.clone(), 0);
    selection.BaseLogicalPlan.SetSchema(schema.Clone());
    selection.SetChildren(vec![Box::new(dual)]);
    let selection_id = selection.ID();

    let group = memo::Convert2Group(Box::new(selection));
    let lines = crate::ToString(ctx.GetExprCtx().GetEvalCtx(), &group);

    assert_eq!(
        lines,
        vec![
            format!(
                "Group#0 Schema:[{}]",
                schema.Columns[0].StringWithCtx(
                    ctx.GetExprCtx().GetEvalCtx(),
                    expression::errors::RedactLogDisable
                )
            ),
            format!("    Selection_{selection_id} input:[Group#1]"),
            format!(
                "Group#1 Schema:[{}]",
                schema.Columns[0].StringWithCtx(
                    ctx.GetExprCtx().GetEvalCtx(),
                    expression::errors::RedactLogDisable
                )
            ),
            format!("    TableDual_{dual_id} rowcount:1"),
        ],
        "Go: preorder Group#0 (Selection) then its child Group#1 (TableDual)"
    );
}

// test_to_string_observes_exploration_rewrite 对应 Go TestGroupStringer 的核心意图：
// exploration 真的改写了 memo，ToString 能看到改写之后的结果，而不是原始输入树。
// 这里用只注册 RuleTransformLimitToTableDual 的规则集，让 count=0 的 Limit 被
// eraseAll 地替换成同 schema 的空 TableDual；改写后 Limit 本身以及它原来的子节点
// 都不会再出现在 ToString 的输出里。
#[test]
fn test_to_string_observes_exploration_rewrite() {
    let ctx = context();
    let schema = one_column_schema();

    let mut dual = LogicalTableDual::default().Init(ctx.clone(), 0);
    dual.RowCount = 1;
    dual.LogicalSchemaProducer.SetSchema(schema.Clone());

    let mut limit = logicalop::LogicalLimit::default().Init(ctx.clone(), 0);
    limit.Count = 0;
    limit.Offset = 0;
    limit.LogicalSchemaProducer.SetSchema(schema.Clone());
    limit.SetChildren(vec![Box::new(dual)]);

    let mut optimizer = Optimizer::NewOptimizer();
    let mut batch = TransformationRuleBatch::new();
    batch.insert(
        pattern::OperandLimit,
        vec![crate::NewRuleTransformLimitToTableDual()],
    );
    optimizer.ResetTransformationRules(vec![batch]);

    let plan: LogicalPlanRef = optimizer
        .onPhasePreprocessing(ctx.as_ref(), Box::new(limit))
        .expect("Go require.NoError");
    let group = memo::Convert2Group(plan);
    optimizer
        .onPhaseExploration(ctx.as_ref(), &group)
        .expect("Go require.NoError");

    let lines = crate::ToString(ctx.GetExprCtx().GetEvalCtx(), &group);
    assert_eq!(
        lines.len(),
        2,
        "the Limit and its old TableDual child are both gone: {lines:?}"
    );
    assert!(lines[0].starts_with("Group#0 Schema:["));
    assert!(
        lines[1].contains("TableDual_") && lines[1].ends_with("rowcount:0"),
        "the count=0 Limit was rewritten into an empty TableDual: {lines:?}"
    );
}

// Go groupExprToString uses ExprNode.ExplainID(), which observes the plan
// context's IgnoreExplainIDSuffix setting instead of always appending `_ID`.
#[test]
fn test_to_string_observes_ignore_explain_id_suffix() {
    let ctx: astersql_planner_core_base::ContextRef = Arc::new(TestPlanContext::new(true));
    let schema = one_column_schema();

    let mut dual = LogicalTableDual::default().Init(ctx.clone(), 0);
    dual.RowCount = 1;
    dual.LogicalSchemaProducer.SetSchema(schema);
    let group = memo::Convert2Group(Box::new(dual));

    let lines = crate::ToString(ctx.GetExprCtx().GetEvalCtx(), &group);
    assert_eq!(lines[1], "    TableDual rowcount:1");
}
