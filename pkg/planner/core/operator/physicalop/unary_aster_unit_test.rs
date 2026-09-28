// Copyright 2026 AsterSQL.

// 一元物理算子（Selection / Projection / Limit / UnionScan）的迁移期单元测试。
//
// 校验这些算子实现 `PhysicalPlan` trait，并验证 Clone 时 Offset/Count、
// 以及部分算子标志位与句柄列（handle columns）的保留语义。

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::PhysicalPlan;

use crate::{PhysicalLimit, PhysicalProjection, PhysicalSelection, PhysicalUnionScan};

/// 测试用 PlanContext：仅实现分配计划 ID 与内建函数计数。
struct TestPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        std::process::abort()
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }
    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("unary operator test does not build ranges")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        std::process::abort()
    }
    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.1.Inc(scalar_func_sig_name)
    }
}

/// 构造带原子计划 ID 计数器的测试上下文。
fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

#[test]
/// 编译期断言一元算子均实现 PhysicalPlan trait。
fn unary_operators_implement_physical_plan() {
    fn assert_plan<T: PhysicalPlan>() {}
    assert_plan::<PhysicalSelection>();
    assert_plan::<PhysicalProjection>();
    assert_plan::<PhysicalLimit>();
    assert_plan::<PhysicalUnionScan>();
}

#[test]
/// 校验 Limit 的 Offset/Count/PrefixLen 在 Clone 后保持一致。
fn limit_preserves_offset_count_and_clone_state() {
    let mut limit = PhysicalLimit::New(context(), 3, 7);
    limit.PrefixLen = 4;
    let cloned = limit.Clone(context()).expect("limit clone");
    assert_eq!((cloned.Offset, cloned.Count, cloned.PrefixLen), (3, 7, 4));
    assert!(limit.GetPartitionBy().is_empty());
}

#[test]
/// 校验 Selection/Projection/UnionScan 克隆时标志位与句柄列语义。
fn unary_clones_keep_operator_flags_and_handles() {
    let mut selection = PhysicalSelection::New(context());
    selection.FromDataSource = true;
    assert!(
        selection
            .Clone(context())
            .expect("selection clone")
            .FromDataSource
    );

    let mut projection = PhysicalProjection::New(context());
    projection.CalculateNoDelay = true;
    projection.AvoidColumnEvaluator = true;
    let cloned = projection.Clone(context()).expect("projection clone");
    assert!(cloned.CalculateNoDelay && cloned.AvoidColumnEvaluator);

    let union_scan = PhysicalUnionScan::New(context());
    let cloned = union_scan.Clone(context()).expect("union scan clone");
    assert_eq!(cloned.HandleCols.NumCols(), union_scan.HandleCols.NumCols());
}
