// Copyright 2026 AsterSQL.

use crate::*;
use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

struct TestPlanContext {
    plan_id: AtomicI32,
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
        panic!("logical limit tests do not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("logical limit tests do not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("logical limit tests do not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("logical limit tests do not perform null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("logical limit tests do not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

#[derive(Default)]
struct LogicalMock {
    base: BaseLogicalPlan,
}

impl LogicalPlan for LogicalMock {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn base(&self) -> &BaseLogicalPlan {
        &self.base
    }

    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.base
    }
}

#[test]
fn limit_to_top_n_matches_go_field_mapping() {
    let mut partition_column = Column::default();
    partition_column.UniqueID = 17;
    let mut limit = LogicalLimit {
        PartitionBy: vec![SortItem {
            Col: partition_column,
            Desc: true,
        }],
        Offset: 3,
        Count: 5,
        PreferLimitToCop: true,
        ..LogicalLimit::default()
    }
    .Init(context(), 9);
    limit.SetChildren(vec![Box::new(LogicalMock::default())]);

    let pushed = limit
        .PushDownTopN(None)
        .expect("limit with one child produces a pushed plan");
    let top_n = pushed
        .as_any()
        .downcast_ref::<LogicalTopN>()
        .expect("base child retains the converted TopN");

    assert_eq!(top_n.Offset, 3);
    assert_eq!(top_n.Count, 5);
    assert!(top_n.PreferLimitToCop);
    assert_eq!(top_n.QueryBlockOffset(), 9);
    assert!(
        top_n.PartitionBy.is_empty(),
        "Go convertToTopN does not copy LogicalLimit.PartitionBy"
    );
}

#[test]
fn logical_plan_hash_code_dispatches_to_limit_semantic_hash() {
    let limit = LogicalLimit {
        Offset: 7,
        Count: 11,
        ..LogicalLimit::default()
    }
    .Init(context(), 9);
    let expected = limit.HashCode().to_vec();
    let plan: LogicalPlanRef = Box::new(limit);

    assert_eq!(LogicalPlan::HashCode(plan.as_ref()), expected);
}
