// Copyright 2026 AsterSQL.

use std::sync::{Arc, atomic::AtomicI32};

use super::NominalSort;

struct TestPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
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

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.1.Inc(scalar_func_sig_name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(1),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

#[test]
fn memory_usage_counts_go_slice_storage_for_by_items() {
    let mut nominal = NominalSort::New(context());
    nominal.ByItems = Vec::with_capacity(4);

    let expected = nominal.PhysicalSchemaProducer.MemoryUsage()
        + std::mem::size_of::<Vec<planner_util::ByItems>>() as i64
        + (nominal.ByItems.capacity() * std::mem::size_of::<*const planner_util::ByItems>()) as i64
        + std::mem::size_of::<bool>() as i64;

    assert_eq!(nominal.MemoryUsage(), expected);
}
