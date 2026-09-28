// Copyright 2026 AsterSQL.

use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use base::{PhysicalPlan as _, PlanContext as _};

use crate::{BasePhysicalAgg, BasePhysicalPlan, PhysicalSchemaProducer, PhysicalStreamAgg};

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
fn get_cost_matches_go_cpu_and_distinct_memory_accounting() {
    let ctx = context();
    let mut aggregate = BasePhysicalAgg::New(PhysicalSchemaProducer::New(BasePhysicalPlan::New(
        ctx.clone(),
        "StreamAgg",
        0,
    )));
    aggregate.AggFuncs.push(
        aggregation::NewAggFuncDesc(ctx.GetExprCtx(), parser_ast::AggFuncCount, Vec::new(), true)
            .expect("build distinct count"),
    );
    let mut stream = PhysicalStreamAgg {
        BasePhysicalAgg: aggregate,
    };
    let input_rows = 40.0;
    let rows_per_group = 5.0;
    stream.set_stats(property::StatsInfo {
        RowCount: input_rows / rows_per_group,
        ..Default::default()
    });
    let vars = ctx.GetSessionVars();
    let expected_root =
        input_rows * vars.GetCPUFactor() * stream.BasePhysicalAgg.GetAggFuncCostFactor(false)
            + rows_per_group * 0.8 * vars.GetMemoryFactor();
    let expected_cop =
        input_rows * vars.GetCopCPUFactor() * stream.BasePhysicalAgg.GetAggFuncCostFactor(false)
            + rows_per_group * 0.8 * vars.GetMemoryFactor();

    assert_eq!(stream.GetCost(input_rows, true, 0), expected_root);
    assert_eq!(stream.GetCost(input_rows, false, 0), expected_cop);
}
