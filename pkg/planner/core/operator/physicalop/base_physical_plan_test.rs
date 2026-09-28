// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::PhysicalPlan;

use crate::{
    BasePhysicalPlan, CollectPlanStatsVersion, PhysicalIndexLookUpReader, PhysicalIndexScan,
    PhysicalTableScan,
};

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

#[test]
fn plan_cost_ver2_is_cached_until_recalculation_is_requested() {
    let mut plan = BasePhysicalPlan::New(context(), "TableDual", 0);
    let plain = costusage::new_default_plan_cost_option();
    let traced =
        costusage::new_default_plan_cost_option().with_cost_flag(costusage::COST_FLAG_TRACE);

    let first = plan
        .GetPlanCostVer2(property::RootTaskType, &plain, &[])
        .unwrap();
    assert!(plan.PlanCostInit);
    assert!(first.get_trace().is_none());

    let cached = plan
        .GetPlanCostVer2(property::RootTaskType, &traced, &[])
        .unwrap();
    assert!(cached.get_trace().is_none());

    let recalculated = plan
        .GetPlanCostVer2(
            property::RootTaskType,
            &traced.with_cost_flag(costusage::COST_FLAG_RECALCULATE | costusage::COST_FLAG_TRACE),
            &[],
        )
        .unwrap();
    assert!(recalculated.get_trace().is_some());
}

fn table(name: &str) -> model::TableInfo {
    let mut table = model::TableInfo::default();
    table.Name = parser_ast::NewCIStr(name);
    table
}

#[test]
fn stats_versions_follow_go_scan_and_index_lookup_rules() {
    let mut index_scan = PhysicalIndexScan::New(context());
    index_scan.Table = Some(table("index_side"));
    let mut index_stats = property::StatsInfo::default();
    index_stats.StatsVersion = 11;
    index_scan.set_stats(index_stats);

    let mut table_scan = PhysicalTableScan::New(context());
    table_scan.Table = Some(table("table_side"));
    let mut table_stats = property::StatsInfo::default();
    table_stats.StatsVersion = 22;
    table_scan.set_stats(table_stats);

    let mut lookup = PhysicalIndexLookUpReader::New(context());
    lookup.IndexPlan = Some(Box::new(index_scan));
    lookup.TablePlan = Some(Box::new(table_scan));

    let mut versions = std::collections::HashMap::new();
    CollectPlanStatsVersion(&lookup, &mut versions);

    assert_eq!(versions.get("index_side"), Some(&11));
    assert!(!versions.contains_key("table_side"));
}
