// Copyright 2026 AsterSQL.

use crate::*;
use std::collections::HashMap;
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
        panic!("unused")
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("unused")
    }
    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("unused")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("unused")
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("unused")
    }
    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.builtin_function_usage.Inc(name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

fn column(id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = id;
    column
}

fn schema(ids: &[i64]) -> Schema {
    expression::NewSchema(ids.iter().copied().map(column).collect())
}

fn child(ctx: base::ContextRef, ids: &[i64]) -> LogicalPlanRef {
    let mut child = MockDataSource::default().Init(ctx);
    child.SetSchema(schema(ids));
    Box::new(child)
}

#[test]
fn prune_with_no_references_preserves_every_union_column() {
    let ctx = context();
    let mut union = LogicalUnionAll::default().Init(ctx.clone(), 2);
    union.SetSchema(schema(&[1, 2]));
    union.SetChildren(vec![child(ctx, &[1, 2])]);

    union.PruneColumns(&[]).unwrap();

    assert_eq!(
        union
            .Schema()
            .Columns
            .iter()
            .map(|c| c.UniqueID)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn residual_predicates_are_attached_to_every_branch() {
    let _ = rule_util::RegisterApplyPredicateSimplification(
        |_context, predicates, _propagate_constant, _filter| predicates,
    );
    let ctx = context();
    let mut union = LogicalUnionAll::default().Init(ctx.clone(), 2);
    union.SetChildren(vec![child(ctx.clone(), &[]), child(ctx, &[])]);

    let remained = union.PredicatePushDown(vec![Box::new(column(9))]).unwrap();

    assert!(remained.is_empty());
    for child in union.Children() {
        let selection = child
            .as_any()
            .downcast_ref::<LogicalSelection>()
            .expect("each child's residual predicate must become a Selection");
        assert_eq!(selection.Conditions.len(), 1);
    }
}

#[test]
fn prune_adds_projection_when_child_keeps_extra_columns() {
    let ctx = context();
    let mut union = LogicalUnionAll::default().Init(ctx.clone(), 2);
    union.SetSchema(schema(&[1, 2]));
    union.SetChildren(vec![child(ctx, &[1, 2])]);

    union.PruneColumns(&[column(1)]).unwrap();

    assert_eq!(union.Schema().Columns.len(), 1);
    let projection = union.Children()[0]
        .as_any()
        .downcast_ref::<LogicalProjection>()
        .expect("extra child columns must be hidden by a Projection");
    assert_eq!(projection.Schema().Columns.len(), 1);
    assert_eq!(projection.Exprs.len(), 1);
}

#[test]
fn top_n_is_cloned_to_each_branch_and_retained_above_union() {
    let ctx = context();
    let mut union = LogicalUnionAll::default().Init(ctx.clone(), 2);
    union.SetChildren(vec![child(ctx.clone(), &[]), child(ctx.clone(), &[])]);
    let upper = LogicalTopN {
        ByItems: vec![ByItems {
            Expr: Box::new(column(7)),
            Desc: true,
        }],
        Offset: 3,
        Count: 5,
        PreferLimitToCop: true,
        ..Default::default()
    }
    .Init(ctx, 9);

    let result = union.PushDownTopN(Some(Box::new(upper))).unwrap();
    let outer = result.as_any().downcast_ref::<LogicalTopN>().unwrap();
    let union = outer.Children()[0]
        .as_any()
        .downcast_ref::<LogicalUnionAll>()
        .unwrap();
    for child in union.Children() {
        let pushed = child.as_any().downcast_ref::<LogicalTopN>().unwrap();
        assert_eq!((pushed.Offset, pushed.Count), (0, 8));
        assert!(pushed.PreferLimitToCop);
        assert!(pushed.ByItems[0].Desc);
    }
}

#[test]
fn stats_use_output_ids_and_do_not_clamp_summed_ndv() {
    let ctx = context();
    let mut union = LogicalUnionAll::default().Init(ctx.clone(), 2);
    union.SetSchema(schema(&[11]));
    let mut left = child(ctx.clone(), &[11]);
    left.SetStats(StatsInfo {
        RowCount: 2.0,
        ColNDVs: HashMap::from([(11, 4.0)]),
        ..Default::default()
    });
    let mut right = child(ctx, &[11]);
    right.SetStats(StatsInfo {
        RowCount: 3.0,
        ColNDVs: HashMap::from([(11, 6.0)]),
        ..Default::default()
    });
    union.SetChildren(vec![left, right]);

    let (stats, fresh) = union.DeriveStats(false).unwrap();

    assert!(fresh);
    assert_eq!(stats.RowCount, 5.0);
    assert_eq!(stats.ColNDVs.get(&11), Some(&10.0));
}

#[test]
fn empty_union_marks_output_not_null_and_caches_fd_set() {
    let mut union = LogicalUnionAll::default().Init(context(), 2);
    union.SetSchema(schema(&[21, 22]));

    let result = union.ExtractFD();

    assert!(result.NotNullCols.Has(21));
    assert!(result.NotNullCols.Has(22));
    assert!(union.base().FDs().is_some());
}
