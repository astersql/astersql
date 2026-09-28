// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::{PhysicalPlan, Plan};

use crate::{PhysicalIndexLookUpReader, PhysicalTableScan, PushedDownLimit, pushedDownLimitSize};

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
fn explain_info_only_reports_embedded_limit_like_go() {
    let mut reader = PhysicalIndexLookUpReader::New(context());
    assert_eq!(reader.ExplainInfo(), "");
    assert_eq!(reader.ExplainNormalizedInfo(), "");

    reader.PushedLimit = Some(PushedDownLimit {
        Offset: 3,
        Count: 17,
    });
    assert_eq!(reader.ExplainInfo(), "limit embedded(offset:3, count:17)");
}

#[test]
fn memory_usage_counts_reader_scalars_and_pushed_limit_like_go() {
    let mut reader = PhysicalIndexLookUpReader::New(context());
    let producer = reader.PhysicalSchemaProducer.MemoryUsage();
    let fixed = (3 * std::mem::size_of::<bool>() + std::mem::size_of::<u64>()) as i64;

    assert_eq!(reader.MemoryUsage(), producer + fixed);

    reader.PushedLimit = Some(PushedDownLimit {
        Offset: 1,
        Count: 2,
    });
    assert_eq!(reader.MemoryUsage(), producer + fixed + pushedDownLimitSize);
}

#[test]
fn init_copies_table_side_stats_like_go() {
    let ctx = context();
    let mut table = PhysicalTableScan::New(ctx.clone());
    let mut stats = property::StatsInfo::default();
    stats.StatsVersion = 29;
    table.set_stats(stats);

    let mut reader = PhysicalIndexLookUpReader::New(ctx.clone());
    reader.TablePlan = Some(Box::new(table));
    let reader = reader.Init(ctx, 7);

    assert_eq!(reader.stats_info().StatsVersion, 29);
}
