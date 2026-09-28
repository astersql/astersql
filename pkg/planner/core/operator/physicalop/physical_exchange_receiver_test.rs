// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::PhysicalExchangeReceiver;

struct TestContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("unused")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("unused")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("unused")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("unused")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("unused")
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

#[test]
fn clone_does_not_copy_runtime_tasks_like_go() {
    let ctx = context();
    let receiver = PhysicalExchangeReceiver::New(ctx.clone());
    receiver.SetTasks(vec![kv::MPPTask {
        ID: 41,
        ..Default::default()
    }]);

    let cloned = receiver.Clone(ctx).expect("clone receiver");

    assert_eq!(receiver.Tasks().len(), 1);
    assert!(cloned.Tasks().is_empty());
}
