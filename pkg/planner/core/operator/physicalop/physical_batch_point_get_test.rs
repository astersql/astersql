// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::{BatchPointGetPlan, PointGetPlan};

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
        std::process::abort()
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        std::process::abort()
    }
    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

#[test]
fn point_get_fixed_cardinality_and_empty_correlations_match_go() {
    let plan = PointGetPlan::New(context());

    assert_eq!(plan.StatsCount(), 1.0);
    assert!(plan.ExtractCorrelatedCols().is_empty());
}

#[test]
fn point_get_operator_info_only_reports_handle_or_lock() {
    let mut plan = PointGetPlan::New(context());

    assert_eq!(plan.OperatorInfo(false), "");
    assert_eq!(plan.OperatorInfo(true), "");
    assert_eq!(plan.ExplainInfo(), "table:unknown");
    plan.Handle = Some(42);
    assert_eq!(plan.OperatorInfo(false), "handle:42");
    assert_eq!(plan.OperatorInfo(true), "handle:?");
    assert_eq!(plan.ExplainNormalizedInfo(), "table:unknown, handle:?");
    plan.Lock = true;
    assert_eq!(plan.OperatorInfo(false), "handle:42, lock");
}

#[test]
fn batch_point_get_operator_info_reports_order_and_lock_flags() {
    let mut plan = BatchPointGetPlan::New(context());

    assert_eq!(
        plan.OperatorInfo(true),
        "handle:?, keep order:false, desc:false"
    );
    assert_eq!(
        plan.ExplainNormalizedInfo(),
        "table:unknown, handle:?, keep order:false, desc:false"
    );
    plan.KeepOrder = true;
    plan.Desc = true;
    plan.Lock = true;
    assert_eq!(
        plan.OperatorInfo(true),
        "handle:?, keep order:true, desc:true, lock"
    );
}
