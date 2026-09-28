// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::ExhaustPhysicalPlans4LogicalMaxOneRow;

struct TestPlanContext {
    plan_id: AtomicI32,
    session_vars: planctx::variable::SessionVars,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.session_vars
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("max-one-row enumeration does not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("max-one-row enumeration does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("max-one-row enumeration does not run null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("max-one-row enumeration does not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

fn enforced_mpp_context() -> base::ContextRef {
    let mut session_vars = planctx::variable::SessionVars::default();
    session_vars.AllowMPPExecution = true;
    session_vars.EnforceMPPExecution = true;
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        session_vars,
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

#[test]
fn mpp_rejection_raises_the_go_warning_when_mpp_is_enforced() {
    let ctx = enforced_mpp_context();
    let logical = logicalop::LogicalMaxOneRow::default().Init(ctx.clone(), 0);
    let required = property::NewPhysicalProperty(property::MppTaskType, &[], false, 0.0, false);

    let plans = ExhaustPhysicalPlans4LogicalMaxOneRow(&logical, &required);

    assert!(plans.is_empty());
    let warnings = ctx.GetSessionVars().StmtCtx.GetExtraWarnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].Err.as_ref().map(ToString::to_string).as_deref(),
        Some("MPP mode may be blocked because operator `MaxOneRow` is not supported now.")
    );
}
