// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::{BasePhysicalJoin, BasePhysicalPlan, PhysicalSchemaProducer};

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

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.1.Inc(scalar_func_sig_name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

fn join() -> BasePhysicalJoin {
    BasePhysicalJoin::New(
        PhysicalSchemaProducer::New(BasePhysicalPlan::New(context(), "HashJoin", 0)),
        base::JoinType::InnerJoin,
    )
}

#[test]
fn ordinary_and_plan_cache_clone_match_go_null_eq_contract() {
    let mut original = join();
    original.IsNullEQ = vec![true, false];

    let ordinary = original.CloneWithSelf(context()).unwrap();
    let cached = original.CloneForPlanCacheWithSelf(context()).unwrap();

    assert!(ordinary.IsNullEQ.is_empty());
    assert_eq!(cached.IsNullEQ, [true, false]);
}

#[test]
fn memory_usage_counts_all_reserved_vector_storage() {
    let baseline = join();
    let baseline_usage = baseline.MemoryUsage();

    let mut reserved = join();
    reserved.LeftConditions.reserve(3);
    reserved.RightConditions.reserve(5);
    reserved.OtherConditions.reserve(7);
    reserved.OuterJoinKeys.reserve(2);
    reserved.InnerJoinKeys.reserve(3);
    reserved.LeftJoinKeys.reserve(4);
    reserved.RightJoinKeys.reserve(5);
    reserved.LeftNAJoinKeys.reserve(6);
    reserved.RightNAJoinKeys.reserve(7);
    reserved.DefaultValues.reserve(8);

    let expression_capacity = reserved.LeftConditions.capacity()
        + reserved.RightConditions.capacity()
        + reserved.OtherConditions.capacity();
    let column_capacity = reserved.OuterJoinKeys.capacity()
        + reserved.InnerJoinKeys.capacity()
        + reserved.LeftJoinKeys.capacity()
        + reserved.RightJoinKeys.capacity()
        + reserved.LeftNAJoinKeys.capacity()
        + reserved.RightNAJoinKeys.capacity();
    let expected_delta = expression_capacity * std::mem::size_of::<expression::ExprBox>()
        + column_capacity * std::mem::size_of::<expression::Column>()
        + reserved.DefaultValues.capacity() * std::mem::size_of::<types::datum::Datum>();

    assert_eq!(
        reserved.MemoryUsage() - baseline_usage,
        expected_delta as i64
    );
}
