// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::PhysicalExchangeSender;

struct TestPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("exchange sender append test does not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("exchange sender append test does not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("exchange sender append test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("exchange sender append test does not run null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("exchange sender append test does not build protobuf executors")
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
fn append_target_tasks_preserves_duplicates_like_go() {
    let mut sender = PhysicalExchangeSender::New(context());
    let task = kv::MPPTask {
        ID: 7,
        GatherID: 11,
        ..kv::MPPTask::default()
    };

    sender.AppendTargetTasks(vec![task.clone(), task]);

    assert_eq!(sender.TargetTasks.len(), 2);
    assert_eq!(sender.TargetTasks[0].ID, 7);
    assert_eq!(sender.TargetTasks[1].ID, 7);
}

#[test]
fn memory_usage_counts_go_slice_headers_and_exchange_type() {
    let sender = PhysicalExchangeSender::New(context());
    let base = sender.PhysicalSchemaProducer.MemoryUsage();

    assert_eq!(
        sender.MemoryUsage(),
        base + (std::mem::size_of::<Vec<kv::MPPTask>>() * 3 + std::mem::size_of::<i32>()) as i64
    );
}
