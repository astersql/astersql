// Copyright 2026 AsterSQL.

// 逻辑计划运行时构建器（LIMIT / DISTINCT）的 Aster 单元测试。
//
// 使用最小 `PlanContext` 桩与 `LogicalTableDual` 作输入，验证
// LIMIT 溢出饱和与零行 Dual、LIMIT 字面量解析，以及 DISTINCT
// 按键前缀建成聚合并拒绝过长键。

use crate::ast;
use crate::logical_plan_builder_runtime::{
    build_distinct_runtime, build_limit_runtime, read_limit_value,
};
use base_dependency as base;
use expression_dependency as expression;
use logicalop::LogicalPlan as _;
use logicalop_dependency as logicalop;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

/// 测试用计划上下文：仅实现分配计划 ID 与表达式上下文，其它路径 panic。
struct TestPlanContext {
    plan_id: AtomicI32,
    expr_ctx: exprstatic_dependency::ExprContext,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        panic!("runtime builder test does not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        &self.expr_ctx
    }

    fn GetRangerCtx(&self) -> &base::RangerContext<'_> {
        panic!("runtime builder test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        &self.expr_ctx
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("runtime builder test does not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

/// 构造带空表达式上下文的测试 `ContextRef`。
fn context() -> base::ContextRef {
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        expr_ctx: exprstatic_dependency::NewExprContext(Vec::new()),
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

/// 构造指定列数的单行 `LogicalTableDual` 作为 LIMIT/DISTINCT 输入。
fn source(ctx: base::ContextRef, columns: usize) -> logicalop::LogicalPlanRef {
    let mut dual = logicalop::LogicalTableDual {
        RowCount: 1,
        ..Default::default()
    }
    .Init(ctx, 7);
    dual.SetSchema(expression::NewSchema(
        (0..columns)
            .map(|index| {
                expression::Column::new(
                    *expression::types::NewFieldType(expression::mysql::TypeLonglong),
                    index as i64 + 1,
                    index as i64 + 1,
                    index as isize,
                )
            })
            .collect(),
    ));
    dual.SetOutputNames(expression::types::NameSlice(vec![
        Some(Arc::clone(
            &expression::types::EmptyName
        ));
        columns
    ]));
    Box::new(dual)
}

/// LIMIT 计数在与 Offset 相加溢出时饱和为 `u64::MAX - offset`；Count=0 退化为零行 Dual。
#[test]
fn limit_saturates_count_and_builds_zero_row_dual() {
    let ctx = context();
    let mut flags = 0;
    let overflow = ast::Limit {
        Count: Some(ast::NewValueExpr(u64::MAX, "", "")),
        Offset: Some(ast::NewValueExpr(10_u64, "", "")),
    };
    let plan = build_limit_runtime(
        ctx.clone(),
        &mut flags,
        None,
        source(ctx.clone(), 1),
        &overflow,
        7,
    )
    .expect("constant LIMIT builds");
    let limit = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalLimit>()
        .expect("nonzero LIMIT remains a LogicalLimit");
    assert_eq!(limit.Offset, 10);
    assert_eq!(limit.Count, u64::MAX - 10);

    let zero = ast::Limit {
        Count: Some(ast::NewValueExpr(0_u64, "", "")),
        Offset: None,
    };
    let plan = build_limit_runtime(
        ctx.clone(),
        &mut flags,
        None,
        source(ctx.clone(), 1),
        &zero,
        7,
    )
    .expect("zero LIMIT builds");
    let dual = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalTableDual>()
        .expect("zero LIMIT becomes a table dual");
    assert_eq!(dual.RowCount, 0);

    // Go's extractLimitCountOffset leaves an omitted Count at zero.  This is
    // observable for parser-recovery/hand-built ASTs and must not become an
    // unbounded LIMIT in Rust.
    let omitted_count = ast::Limit {
        Count: None,
        Offset: Some(ast::NewValueExpr(4_u64, "", "")),
    };
    let plan = build_limit_runtime(
        ctx.clone(),
        &mut flags,
        None,
        source(ctx, 1),
        &omitted_count,
        7,
    )
    .expect("omitted count follows Go's zero value");
    let limit = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalLimit>()
        .expect("nonzero offset remains a LogicalLimit");
    assert_eq!(limit.Offset, 4);
    assert_eq!(limit.Count, 0);
}

/// `read_limit_value` 仅接受无符号或非负有符号整数，拒绝负数与浮点。
#[test]
fn limit_value_accepts_only_parser_unsigned_or_non_negative_signed_integers() {
    let unsigned = ast::NewValueExpr(9_u64, "", "");
    assert_eq!(read_limit_value(Some(&unsigned), 0).unwrap(), 9);

    let signed = ast::NewValueExpr(7_i64, "", "");
    assert_eq!(read_limit_value(Some(&signed), 0).unwrap(), 7);
    assert_eq!(read_limit_value(None, 11).unwrap(), 11);

    let negative = ast::NewValueExpr(-1_i64, "", "");
    assert!(read_limit_value(Some(&negative), 0).is_err());

    let fractional = ast::NewValueExpr(1.5_f64, "", "");
    assert!(read_limit_value(Some(&fractional), 0).is_err());
}

/// DISTINCT 按请求键前缀建成 FirstRow 聚合；键长超过 Schema 时报错。
#[test]
fn distinct_uses_requested_key_prefix_and_rejects_oversized_key() {
    let ctx = context();
    let mut flags = 0;
    let plan = build_distinct_runtime(ctx.clone(), &mut flags, None, source(ctx.clone(), 2), 1)
        .expect("valid distinct key builds");
    let aggregation = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
        .expect("DISTINCT becomes aggregation");
    assert_eq!(aggregation.GroupByItems.len(), 1);
    assert_eq!(aggregation.AggFuncs.len(), 2);
    assert!(
        aggregation
            .AggFuncs
            .iter()
            .all(|function| function.Name == ast::AggFuncFirstRow
                && function.Args.len() == 1
                && function.RetTp.is_some())
    );

    let error = build_distinct_runtime(ctx.clone(), &mut flags, None, source(ctx, 2), 3)
        .err()
        .expect("oversized distinct key is rejected");
    assert!(error.to_string().contains("key length"));
}
