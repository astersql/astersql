// Copyright 2026 AsterSQL.

use super::contextimpl::newPlanContextImpl;
use astersql_planner_plannersession::{ExprContext, PlannerSessionError, SessionContext};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestExprContext;

impl ExprContext for TestExprContext {}

struct TestSession {
    expr_context: Arc<dyn ExprContext>,
    warmups: AtomicUsize,
}

impl SessionContext for TestSession {
    fn GetExprCtx(&self) -> Arc<dyn ExprContext> {
        Arc::clone(&self.expr_context)
    }

    fn AdviseTxnWarmup(&self) -> Result<(), PlannerSessionError> {
        self.warmups.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn new_plan_context_impl_preserves_session_and_builds_real_extension() {
    let session = Arc::new(TestSession {
        expr_context: Arc::new(TestExprContext),
        warmups: AtomicUsize::new(0),
    });
    let session_context: Arc<dyn SessionContext> = session.clone();

    let mut context = newPlanContextImpl(Arc::clone(&session_context));

    assert!(Arc::ptr_eq(&context.session, &session_context));
    assert!(
        context
            .plan_ctx_extended
            .GetNullRejectCheckExprCtx()
            .IsInNullRejectCheck()
    );
    context.plan_ctx_extended.AdviseTxnWarmup().unwrap();
    assert_eq!(session.warmups.load(Ordering::SeqCst), 1);

    context
        .plan_ctx_extended
        .SetReadonlyUserVarMap(HashMap::from([("answer".to_owned(), ())]));
    assert!(
        context
            .plan_ctx_extended
            .GetReadonlyUserVarMap()
            .is_some_and(|variables| variables.contains_key("answer"))
    );
    context.plan_ctx_extended.Reset();
    assert!(context.plan_ctx_extended.GetReadonlyUserVarMap().is_none());
}
