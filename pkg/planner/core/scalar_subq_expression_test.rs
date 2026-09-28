// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base_dependency as base;

use crate::{ScalarSubQueryExpr, ScalarSubqueryEvalCtx};
use physicalop_dependency::PhysicalTableDual;

struct TestPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        std::process::abort()
    }

    fn GetExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        std::process::abort()
    }

    fn GetRangerCtx(&self) -> &base::RangerContext<'_> {
        std::process::abort()
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        std::process::abort()
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        std::process::abort()
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

#[test]
fn string_uses_expression_column_id_even_when_context_has_other_outputs() {
    let plan_context: base::ContextRef = Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ));
    let physical_plan: Arc<dyn base::PhysicalPlan> =
        Arc::new(PhysicalTableDual::New(Arc::clone(&plan_context), 1));
    let mut eval_context = ScalarSubqueryEvalCtx::New(
        plan_context,
        0,
        physical_plan,
        crate::context::BackgroundArc(),
        infoschema_dependency::infoschema::MockInfoSchema(Vec::new()),
    );
    eval_context.output_col_ids = vec![99];

    let expression = ScalarSubQueryExpr::new(-42, eval_context);

    assert_eq!(expression.String(), "ScalarQueryCol#-42");
}
