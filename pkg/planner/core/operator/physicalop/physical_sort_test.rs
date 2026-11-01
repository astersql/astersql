// Copyright 2026 AsterSQL.

use std::sync::{Arc, atomic::AtomicI32};

use base::{PhysicalPlan as _, Plan as _};
use logicalop::LogicalPlan as _;

use crate::{ExhaustPhysicalPlans4LogicalSort, NominalSort, PhysicalSort, PhysicalTableDual};

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
fn memory_usage_counts_go_slice_storage_for_by_items() {
    let mut sort = PhysicalSort::New(context());
    sort.ByItems = Vec::with_capacity(4);

    let expected = sort.PhysicalSchemaProducer.MemoryUsage()
        + std::mem::size_of::<Vec<planner_util::ByItems>>() as i64
        + (sort.ByItems.capacity() * std::mem::size_of::<*const planner_util::ByItems>()) as i64
        + std::mem::size_of::<bool>() as i64;

    assert_eq!(sort.MemoryUsage(), expected);
}

#[test]
fn root_sort_enumerates_physical_and_nominal_candidates_like_go() {
    let column = expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        1,
        1,
        0,
    );
    let mut logical = logicalop::LogicalSort {
        ByItems: vec![planner_util::ByItems {
            Expr: Box::new(column.Clone()),
            Desc: false,
        }],
        ..Default::default()
    }
    .Init(context(), 0);
    logical.SetSchema(expression::NewSchema(vec![column.Clone()]));

    let mut required = property::PhysicalProperty::default();
    required.TaskTp = property::RootTaskType;
    required.SortItems.push(property::SortItem {
        Col: column,
        Desc: false,
    });

    let plans = ExhaustPhysicalPlans4LogicalSort(&logical, &required);

    assert_eq!(plans.len(), 2);
    assert!(plans[0].as_any().is::<PhysicalSort>());
    assert!(plans[1].as_any().is::<NominalSort>());
}

#[test]
fn sort_replacement_preserves_child_output_schema() {
    let ctx = context();
    let column = |id| expression::Column::new(expression::types::FieldType::default(), id, id, 0);
    let child = |ids: &[i64]| {
        let mut plan = PhysicalTableDual::New(ctx.clone(), 1);
        plan.PhysicalSchemaProducer.SetSchema(expression::NewSchema(
            ids.iter().copied().map(column).collect(),
        ));
        Box::new(plan) as Box<dyn base::PhysicalPlan>
    };
    let mut sort = PhysicalSort::New(ctx.clone());
    sort.PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![column(7), column(8), column(9)]));
    sort.set_children(vec![child(&[7, 8])]);
    assert_eq!(
        sort.schema()
            .Columns
            .iter()
            .map(|c| c.UniqueID)
            .collect::<Vec<_>>(),
        vec![7, 8]
    );

    sort.set_child(0, child(&[10]));
    assert_eq!(
        sort.schema()
            .Columns
            .iter()
            .map(|c| c.UniqueID)
            .collect::<Vec<_>>(),
        vec![10]
    );
}

#[test]
fn port_87ffb1fda6_reverse_nullable_sort_properties() {
    let ctx = exprstatic::NewExprContext(vec![]);
    for not_null in [false, true] {
        for desc in [false, true] {
            for reversals in [1, 2] {
                let mut tp = *expression::types::NewFieldType(expression::mysql::TypeDouble);
                if not_null {
                    tp.AddFlag(expression::mysql::NotNullFlag);
                }
                let column = expression::Column::new(tp.clone(), 1, 1, 0);
                let mut expr: Box<dyn expression::Expression> = Box::new(column);
                for _ in 0..reversals {
                    expr = expression::NewFunctionBase(
                        &ctx,
                        expression::ast::UnaryMinus,
                        tp.clone(),
                        vec![expr],
                    )
                    .unwrap();
                }
                let (prop, only_columns) =
                    crate::GetPropByOrderByItemsContainScalarFunc(&[planner_util::ByItems {
                        Expr: expr,
                        Desc: desc,
                    }]);
                assert!(!only_columns);
                assert_eq!(
                    prop.is_some(),
                    not_null || reversals == 2,
                    "not_null={not_null}, desc={desc}, reversals={reversals}"
                );
                if let Some(prop) = prop {
                    assert_eq!(
                        prop.SortItems[0].Desc,
                        if reversals == 1 { !desc } else { desc }
                    );
                }
            }
        }
    }
}
