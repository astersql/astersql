// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use baseimpl::Plan;
use types::metadata::{FieldName, NameSlice};

use crate::{NewBasePhysicalPlan, PhysicalSchemaProducer, SimpleSchemaProducer};

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
fn plan_cache_clones_share_schema_like_go() {
    let ctx = context();
    let mut physical = PhysicalSchemaProducer::New(NewBasePhysicalPlan(
        Arc::clone(&ctx),
        "PhysicalSchemaProducerTest",
        0,
    ));
    physical.SetSchema(expression::NewSchema(Vec::new()));
    let physical_clone = physical
        .CloneForPlanCacheWithSelf(Arc::clone(&ctx))
        .expect("base plan is cloneable");
    assert!(std::ptr::eq(
        physical.SchemaRef().expect("schema set"),
        physical_clone.SchemaRef().expect("schema cloned"),
    ));

    let mut simple = SimpleSchemaProducer::New(Arc::clone(&ctx), "SimpleSchemaProducerTest", 0);
    simple.SetSchema(expression::NewSchema(Vec::new()));
    let simple_clone = simple.CloneSelfForPlanCache(ctx);
    assert!(std::ptr::eq(
        simple.SchemaRef().expect("schema set"),
        simple_clone.SchemaRef().expect("schema cloned"),
    ));
}

#[test]
fn memory_usage_matches_go_field_accounting() {
    let ctx = context();
    let mut physical = PhysicalSchemaProducer::New(NewBasePhysicalPlan(
        Arc::clone(&ctx),
        "PhysicalSchemaProducerTest",
        0,
    ));
    physical.SetSchema(expression::NewSchema(Vec::new()));
    assert_eq!(
        physical.MemoryUsage(),
        physical.BasePhysicalPlan.MemoryUsage() + std::mem::size_of::<usize>() as i64,
    );

    let mut names = Vec::with_capacity(3);
    names.push(Some(Arc::new(FieldName::default())));
    let mut simple = SimpleSchemaProducer::New(ctx, "SimpleSchemaProducerTest", 0);
    simple.SetOutputNames(NameSlice(names));
    simple.SetSchema(expression::NewSchema(Vec::new()));
    let expected = simple.Plan.MemoryUsage()
        + std::mem::size_of::<usize>() as i64
        + std::mem::size_of::<Vec<usize>>() as i64
        + 3 * std::mem::size_of::<usize>() as i64
        + simple.SchemaRef().expect("schema set").MemoryUsage()
        + FieldName::default().MemoryUsage();
    assert_eq!(simple.MemoryUsage(), expected);
}
