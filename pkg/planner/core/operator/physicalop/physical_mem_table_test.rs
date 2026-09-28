// Copyright 2026 AsterSQL.

use crate::PhysicalMemTable;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

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
fn explain_info_uses_go_scan_access_object_format() {
    let mut plan = PhysicalMemTable::New(context());
    plan.DBName = parser_ast::NewCIStr("INFORMATION_SCHEMA");
    plan.Table.Name = parser_ast::NewCIStr("TABLES");

    assert_eq!(plan.ExplainInfo(), "table:TABLES");
}

#[test]
fn memory_usage_counts_go_field_headers_and_column_pointers() {
    let mut plan = PhysicalMemTable::New(context());
    plan.DBName = parser_ast::NewCIStr("db");
    plan.Columns = Vec::with_capacity(3);
    plan.QueryTimeRange = Some((1, 2));

    let pointer = std::mem::size_of::<*const ()>() as i64;
    let slice = std::mem::size_of::<[usize; 3]>() as i64;
    let interface = std::mem::size_of::<[usize; 2]>() as i64;
    let expected = plan.PhysicalSchemaProducer.MemoryUsage()
        + plan.DBName.memory_usage()
        + pointer
        + slice
        + 3 * pointer
        + interface
        + std::mem::size_of::<Option<(i64, i64)>>() as i64;

    assert_eq!(plan.MemoryUsage(), expected);
}
