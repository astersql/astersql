// Copyright 2026 AsterSQL.

use crate::*;
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
        panic!("partition union tests do not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("partition union tests do not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("partition union tests do not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("partition union tests do not perform null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("partition union tests do not build protobuf executors")
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

#[test]
fn push_down_top_n_clones_limit_and_order_for_every_partition() {
    let ctx = context();
    let mut partition = LogicalPartitionUnionAll::default().Init(ctx.clone(), 3);
    partition.SetChildren(vec![
        Box::new(MockDataSource::default().Init(ctx.clone())),
        Box::new(MockDataSource::default().Init(ctx.clone())),
    ]);
    let mut order_column = Column::default();
    order_column.UniqueID = 42;
    let upper = LogicalTopN {
        ByItems: vec![ByItems {
            Expr: Box::new(order_column),
            Desc: true,
        }],
        Offset: 3,
        Count: 5,
        PreferLimitToCop: true,
        ..LogicalTopN::default()
    }
    .Init(ctx, 9);

    let result = partition
        .PushDownTopN(Some(Box::new(upper)))
        .expect("partition union remains in the returned tree");
    let outer = result
        .as_any()
        .downcast_ref::<LogicalTopN>()
        .expect("the original TopN remains above the partition union");
    assert_eq!((outer.Offset, outer.Count), (3, 5));
    let union = outer.Children()[0]
        .as_any()
        .downcast_ref::<LogicalPartitionUnionAll>()
        .expect("outer TopN must attach to the partition union");
    assert_eq!(union.Children().len(), 2);
    for child in union.Children() {
        let pushed = child
            .as_any()
            .downcast_ref::<LogicalTopN>()
            .expect("each partition receives a cloned TopN");
        assert_eq!((pushed.Offset, pushed.Count), (0, 8));
        assert!(pushed.PreferLimitToCop);
        assert_eq!(pushed.ByItems.len(), 1);
        assert!(pushed.ByItems[0].Desc);
    }
}

#[test]
fn push_down_without_top_n_preserves_partition_union() {
    let mut partition = LogicalPartitionUnionAll::default().Init(context(), 3);
    partition.SetChildren(vec![Box::new(MockDataSource::default())]);

    let result = partition
        .PushDownTopN(None)
        .expect("Go returns the partition union when no TopN is supplied");

    assert!(result.as_any().is::<LogicalPartitionUnionAll>());
}
