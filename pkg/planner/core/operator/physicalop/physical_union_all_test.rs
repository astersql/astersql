// Copyright 2026 AsterSQL.

use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use base::PhysicalPlan as _;

use crate::PhysicalUnionAll;

struct TestPlanContext {
    plan_id: AtomicI32,
    vars: planctx::variable::SessionVars,
    expr: exprstatic::ExprContext,
    usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.vars
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        &self.expr
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        std::process::abort()
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        &self.expr
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        std::process::abort()
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.usage.Inc(name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        vars: Default::default(),
        expr: exprstatic::NewExprContext(Vec::new()),
        usage: Default::default(),
    })
}

#[test]
fn v1_cost_includes_go_union_worker_overhead() {
    let ctx = context();
    let mut union = PhysicalUnionAll::New(ctx.clone());
    let cost = union
        .GetPlanCostVer1(
            property::RootTaskType,
            &costusage::new_default_plan_cost_option(),
        )
        .unwrap();

    assert_eq!(cost, vardef::DefOptConcurrencyFactor);

    union.set_children(vec![
        Box::new(PhysicalUnionAll::New(ctx.clone())),
        Box::new(PhysicalUnionAll::New(ctx)),
    ]);
    let recalculated = union
        .GetPlanCostVer1(
            property::RootTaskType,
            &costusage::new_default_plan_cost_option()
                .with_cost_flag(costusage::COST_FLAG_RECALCULATE),
        )
        .unwrap();
    assert_eq!(recalculated, 4.0 * vardef::DefOptConcurrencyFactor);
}
