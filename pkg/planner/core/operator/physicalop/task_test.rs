// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::{PhysicalPlan, Plan};

use crate::{
    BasePhysicalPlan, CopTask, PhysicalSelection, PhysicalTableScan, RootTask,
    TryExpandVirtualColumn,
};

struct TestPlanContext {
    plan_id: AtomicI32,
    session: planctx::variable::SessionVars,
    builtin_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.session
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
        self.builtin_usage.Inc(name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        session: planctx::variable::SessionVars::default(),
        builtin_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

#[test]
fn get_store_type_reads_the_leaf_table_scan_like_go() {
    let mut scan = PhysicalTableScan::New(context());
    scan.StoreType = kv::StoreType::TiFlash;
    let task = CopTask {
        TablePlan: Some(Box::new(scan)),
        ..Default::default()
    };

    assert_eq!(task.GetStoreType(), kv::StoreType::TiFlash);
}

#[test]
fn finish_index_plan_copies_stats_but_preserves_table_version_like_go() {
    let mut table = PhysicalTableScan::New(context());
    table.set_stats(property::StatsInfo {
        RowCount: 100.0,
        StatsVersion: 7,
        ..Default::default()
    });
    let mut index = PhysicalTableScan::New(context());
    index.set_stats(property::StatsInfo {
        RowCount: 12.0,
        StatsVersion: 99,
        ..Default::default()
    });
    let mut task = CopTask {
        TablePlan: Some(Box::new(table)),
        IndexPlan: Some(Box::new(index)),
        ..Default::default()
    };

    task.FinishIndexPlan();

    let stats = task.TablePlan.as_ref().unwrap().stats_info();
    assert_eq!(stats.RowCount, 12.0);
    assert_eq!(stats.StatsVersion, 7);
}

#[test]
fn root_conditions_materialize_a_selection_like_go() {
    property::SetScaleNDVFunc(Some(|_, ndv, rows, selected| {
        if rows == 0.0 {
            ndv
        } else {
            ndv * selected / rows
        }
    }));
    let ctx = context();
    let mut scan = PhysicalTableScan::New(ctx.clone());
    scan.set_stats(property::StatsInfo {
        RowCount: 20.0,
        ..Default::default()
    });
    let mut root = RootTask::New(Box::new(scan), None);
    let task = CopTask {
        RootTaskConds: vec![Box::new(expression::NewInt64Const(1))],
        ..Default::default()
    };

    task.HandleRootTaskConds(&mut root, Some(0.25));

    let selection = root
        .GetPlan()
        .as_any()
        .downcast_ref::<PhysicalSelection>()
        .unwrap();
    assert!(selection.FromDataSource);
    assert_eq!(selection.Conditions.len(), 1);
    assert_eq!(selection.children().len(), 1);
    assert_eq!(selection.stats_info().RowCount, 5.0);
}

#[test]
fn virtual_column_walk_visits_concrete_children_without_replacing_them() {
    let ctx = context();
    let scan = PhysicalTableScan::New(ctx.clone());
    let mut root = BasePhysicalPlan::New(ctx, "root", 0);
    root.SetChildren(vec![Box::new(scan)]);
    let original_child_id = root.Children()[0].id();
    let mut visited = Vec::new();

    TryExpandVirtualColumn(&mut root, &mut |plan| {
        visited.push(plan.tp(&[]));
        false
    });

    assert_eq!(visited, ["root", plancodec::TypeTableScan]);
    assert_eq!(root.Children()[0].id(), original_child_id);
}
