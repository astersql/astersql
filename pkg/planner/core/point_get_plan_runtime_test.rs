// Copyright 2026 AsterSQL.

use crate::main_test::build_logical_for_test;
use crate::point_get_plan_runtime::TryFastIntegerPointGet;
use base_dependency as base;
use expression_dependency as expression;
use physicalop_dependency::PointGetPlan;
use std::sync::Arc;

// The shared builder fixture snapshots IDs but inherits a no-op reset. Supply
// the same reset contract as SessionPlanContext without changing other suites.
struct ResettingContext(base::ContextRef);
impl base::PlanContext for ResettingContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.alloc_plan_id()
    }
    fn reset_plan_id(&self) {
        self.0.restore_plan_id_checkpoint(0);
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        self.0.ignore_explain_id_suffix()
    }
    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        self.0.GetSessionVars()
    }
    fn GetExprCtx(&self) -> &dyn expression::exprctx::ExprContext {
        self.0.GetExprCtx()
    }
    fn GetRangerCtx(&self) -> &base::RangerContext<'_> {
        self.0.GetRangerCtx()
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn expression::exprctx::ExprContext {
        self.0.GetNullRejectCheckExprCtx()
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        self.0.GetBuildPBCtx()
    }
    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.0.BuiltinFunctionUsageInc(name);
    }
}

#[test]
fn integer_point_get_keeps_handle_projection_and_resets_id() {
    for sql in [
        "select b from t where a = 7",
        "select b as value, a from t where 7 = a",
    ] {
        let (context, logical) = build_logical_for_test(sql).unwrap();
        let context: base::ContextRef = Arc::new(ResettingContext(context));
        for _ in 0..2 {
            let plan = TryFastIntegerPointGet(&context, logical.as_ref()).expect(sql);
            assert_eq!(plan.id(), 1);
            let point = plan.as_any().downcast_ref::<PointGetPlan>().unwrap();
            assert_eq!(point.Handle, Some(7));
            assert_eq!(point.Columns[0].Name.L, "b");
            assert_eq!(point.Columns.len(), logical.Schema().Columns.len());
            assert_eq!(plan.output_names().0.len(), logical.OutputNames().0.len());
        }
    }
}

#[test]
fn integer_point_get_preserves_general_optimizer_fallbacks() {
    for sql in [
        "select b + 1 from t where a = 7",
        "select b from t where a = 7 and b = 9",
        "select b from t where a > 7",
        "select b from t where b = 7",
        "select b from t where a is null",
        "select b from t where a = 7 order by b",
        "select b from t where a = 7 limit 0",
        "select b from t3 where a = 7",
        "select b from t2 where a = -1",
    ] {
        let (context, logical) = build_logical_for_test(sql).unwrap();
        assert!(
            TryFastIntegerPointGet(&context, logical.as_ref()).is_none(),
            "{sql}"
        );
    }
}
