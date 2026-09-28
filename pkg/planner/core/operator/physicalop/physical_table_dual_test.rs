// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::Plan as _;
use types::metadata::{FieldName, NameSlice};

use crate::PhysicalTableDual;

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
fn output_names_clone_and_memory_match_go_owned_names_contract() {
    let ctx = context();
    let mut names = Vec::with_capacity(3);
    let field = Arc::new(FieldName::default());
    names.push(Some(Arc::clone(&field)));

    let mut dual = PhysicalTableDual::New(Arc::clone(&ctx), 1);
    dual.set_output_names(NameSlice(names));

    let output = dual.output_names();
    assert_eq!(output.0.len(), 1);
    assert!(Arc::ptr_eq(
        output.0[0].as_ref().expect("field name"),
        &field
    ));

    let cloned = dual.Clone(ctx).expect("clone dual");
    let cloned_output = cloned.output_names();
    assert_eq!(cloned_output.0.len(), 1);
    assert!(!Arc::ptr_eq(
        cloned_output.0[0].as_ref().expect("cloned field name"),
        &field,
    ));

    let expected = dual.PhysicalSchemaProducer.MemoryUsage()
        + std::mem::size_of::<i32>() as i64
        + std::mem::size_of::<Vec<usize>>() as i64
        + 3 * std::mem::size_of::<usize>() as i64
        + field.MemoryUsage();
    assert_eq!(dual.MemoryUsage(), expected);
}

#[test]
fn init_reuses_the_constructor_plan_id() {
    let ctx = context();
    let dual = PhysicalTableDual::New(Arc::clone(&ctx), 1);
    let constructor_id = dual.id();

    let dual = dual.Init(Arc::clone(&ctx), property::StatsInfo::default(), 7);
    assert_eq!(dual.id(), constructor_id);
    assert_eq!(dual.query_block_offset(), 7);

    let next = PhysicalTableDual::New(ctx, 0);
    assert_eq!(next.id(), constructor_id + 1);
}
