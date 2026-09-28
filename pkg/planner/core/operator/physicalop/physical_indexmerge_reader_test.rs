// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::{PhysicalPlan, Plan};
use expression::{Column, Schema};

use crate::{PhysicalIndexMergeReader, PhysicalIndexScan, PhysicalTableScan, PushedDownLimit};

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

fn schema_with_id(id: i64) -> Schema {
    let mut column = Column::default();
    column.ID = id;
    column.UniqueID = id;
    expression::NewSchema(vec![column])
}

#[test]
fn explain_info_matches_go_merge_type_and_embedded_limit() {
    let mut reader = PhysicalIndexMergeReader::New(context());
    assert_eq!(reader.ExplainInfo(), "type: union");
    assert_eq!(reader.ExplainNormalizedInfo(), "");

    reader.IsIntersectionType = true;
    reader.PushedLimit = Some(PushedDownLimit {
        Offset: 3,
        Count: 17,
    });
    assert_eq!(
        reader.ExplainInfo(),
        "type: intersection, limit embedded(offset:3, count:17)"
    );
}

#[test]
fn init_without_table_uses_index_data_source_schema_and_first_stats() {
    let ctx = context();
    let mut scan = PhysicalIndexScan::New(ctx.clone());
    scan.DataSourceSchema = Some(schema_with_id(41));
    let mut stats = property::StatsInfo::default();
    stats.RowCount = 1.0;
    stats.StatsVersion = 29;
    scan.set_stats(stats);

    let mut reader = PhysicalIndexMergeReader::New(ctx.clone());
    reader.PartialPlansRaw.push(Box::new(scan));
    let reader = reader.Init(ctx, 7);

    assert_eq!(reader.schema().Columns[0].UniqueID, 41);
    assert_eq!(reader.stats_info().StatsVersion, 29);
}

#[test]
fn memory_usage_counts_flattened_and_raw_plans_like_go() {
    let ctx = context();
    let scan = PhysicalTableScan::New(ctx.clone());
    let scan_memory = scan.MemoryUsage();
    let mut reader = PhysicalIndexMergeReader::New(ctx);
    let producer_memory = reader.PhysicalSchemaProducer.MemoryUsage();
    reader.PartialPlansRaw.push(Box::new(scan));

    assert_eq!(reader.MemoryUsage(), producer_memory + 2 * scan_memory);
}
