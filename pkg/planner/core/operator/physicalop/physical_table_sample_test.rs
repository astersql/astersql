// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::Plan as _;

use crate::physical_table_sample::PhysicalTableSample;

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
fn init_and_memory_usage_match_go_contract() {
    let ctx = context();
    let sample_info = Arc::new(tablesampler::TableSampleInfo {
        AstNode: None,
        FullSchema: None,
        Partitions: Vec::with_capacity(3),
    });
    let sample = PhysicalTableSample::New(Arc::clone(&ctx), 42, true)
        .WithTableSampleInfo(Arc::clone(&sample_info));
    let constructor_id = sample.id();

    let sample = sample.Init(ctx, 7);
    assert_eq!(sample.id(), constructor_id);
    assert_eq!(sample.query_block_offset(), 7);
    assert_eq!(sample.stats_info().RowCount, 1.0);
    assert_eq!(sample.PhysicalTableID, 42);
    assert!(sample.Desc);

    let expected = sample.PhysicalSchemaProducer.MemoryUsage()
        + std::mem::size_of::<Option<Arc<dyn table::Table>>>() as i64
        + std::mem::size_of::<bool>() as i64
        + sample_info.MemoryUsage();
    assert_eq!(sample.MemoryUsage(), expected);
}
