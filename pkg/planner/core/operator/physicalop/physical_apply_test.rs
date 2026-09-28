// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::{JoinType, PhysicalPlan, Plan};

use crate::{
    BasePhysicalJoin, BasePhysicalPlan, NewPhysicalHashJoin, PhysicalApply, PhysicalSchemaProducer,
    PhysicalTableScan,
};

struct TestPlanContext {
    plan_id: AtomicI32,
    session_vars: planctx::variable::SessionVars,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.session_vars
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
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        session_vars: planctx::variable::SessionVars::default(),
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

fn scan(ctx: base::ContextRef, columns: Vec<expression::Column>) -> PhysicalTableScan {
    let mut scan = PhysicalTableScan::New(ctx);
    scan.PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(columns));
    scan
}

fn apply(ctx: base::ContextRef) -> PhysicalApply {
    let producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(ctx, "Apply", 0));
    let join = BasePhysicalJoin::New(producer, JoinType::InnerJoin);
    PhysicalApply::New(NewPhysicalHashJoin(join, 1, false))
}

fn column(id: i64, unique_id: i64, index: isize) -> expression::Column {
    expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        id,
        unique_id,
        index,
    )
}

#[test]
fn resolve_indices_deduplicates_and_resolves_outer_schema_columns() {
    let ctx = context();
    let first = column(1, 11, 0);
    let target = column(2, 22, 1);
    let inner = column(3, 33, 0);

    let mut apply = apply(ctx.clone());
    apply.set_children(vec![
        Box::new(scan(ctx.clone(), vec![first, target.Clone()])),
        Box::new(scan(ctx, vec![inner])),
    ]);
    let mut first_duplicate = expression::CorrelatedColumn {
        column: column(2, 22, -1),
        data: None,
    };
    first_duplicate.column.OrigName = "first duplicate".to_owned();
    let mut last_duplicate = first_duplicate.Clone();
    last_duplicate.column.OrigName = "last duplicate".to_owned();
    apply.OuterSchema = vec![first_duplicate, last_duplicate];

    apply.ResolveIndices().unwrap();

    assert_eq!(apply.OuterSchema.len(), 1);
    assert_eq!(apply.OuterSchema[0].column.UniqueID, target.UniqueID);
    assert_eq!(apply.OuterSchema[0].column.Index, 1);
    assert_eq!(apply.OuterSchema[0].column.OrigName, "last duplicate");
}

#[test]
fn get_cost_matches_go_filter_and_semi_join_accounting() {
    let ctx = context();
    let cpu = ctx.GetSessionVars().GetCPUFactor();
    let mut apply = apply(ctx);
    apply.PhysicalHashJoin.BasePhysicalJoin.JoinType = JoinType::SemiJoin;
    apply
        .PhysicalHashJoin
        .BasePhysicalJoin
        .LeftConditions
        .push(Box::new(column(1, 1, 0)));
    apply
        .PhysicalHashJoin
        .BasePhysicalJoin
        .RightConditions
        .push(Box::new(column(2, 2, 0)));
    apply
        .PhysicalHashJoin
        .BasePhysicalJoin
        .OtherConditions
        .push(Box::new(column(3, 3, 0)));

    let actual = apply.GetCost(10.0, 5.0, 7.0, 3.0);
    let filtered_left = 10.0 * cardinality::SelectionFactor;
    let filtered_right = 5.0 * cardinality::SelectionFactor;
    let expected = 10.0 * cpu
        + filtered_left * 5.0 * cpu
        + filtered_left * filtered_right * cpu * 0.5
        + 7.0
        + filtered_left * 3.0;

    assert_eq!(actual, expected);
}

#[test]
fn attach_to_task_builds_the_join_output_schema() {
    let ctx = context();
    let left = scan(ctx.clone(), vec![column(1, 11, 0)]);
    let right = scan(ctx.clone(), vec![column(2, 22, 0)]);
    let apply = apply(ctx);

    let task = apply.Attach2Task(vec![
        Box::new(crate::RootTask::New(Box::new(left), None)),
        Box::new(crate::RootTask::New(Box::new(right), None)),
    ]);

    assert_eq!(task.plan().schema().Len(), 2);
    assert_eq!(task.plan().schema().Columns[0].UniqueID, 11);
    assert_eq!(task.plan().schema().Columns[1].UniqueID, 22);
}
