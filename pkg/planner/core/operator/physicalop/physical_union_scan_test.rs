// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::Plan as _;

use crate::PhysicalUnionScan;

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
fn init_reuses_constructor_plan_id_like_go_init() {
    let ctx = context();
    let union_scan = PhysicalUnionScan::New(Arc::clone(&ctx));
    let constructor_id = union_scan.id();

    let union_scan = union_scan.Init(
        Arc::clone(&ctx),
        property::StatsInfo::default(),
        7,
        Vec::new(),
    );
    assert_eq!(union_scan.id(), constructor_id);
    assert_eq!(union_scan.query_block_offset(), 7);

    let next = PhysicalUnionScan::New(ctx);
    assert_eq!(next.id(), constructor_id + 1);
}

#[test]
fn memory_usage_includes_go_conditions_slice_header() {
    let union_scan = PhysicalUnionScan::New(context());
    let expected = union_scan.PhysicalSchemaProducer.MemoryUsage()
        + std::mem::size_of::<Vec<expression::ExprBox>>() as i64
        + union_scan.HandleCols.MemoryUsage();
    assert_eq!(union_scan.MemoryUsage(), expected);
}
