// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::Plan as _;

use crate::{PhysicalIndexReader, PhysicalIndexScan, PhysicalProjection};

struct TestPlanContext {
    next_id: AtomicI32,
    builtin_usage: base::BuiltinFunctionUsageCounter,
    expr_ctx: exprstatic::ExprContext,
}

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.next_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        std::process::abort()
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        &self.expr_ctx
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
        next_id: AtomicI32::new(0),
        builtin_usage: base::BuiltinFunctionUsageCounter::default(),
        expr_ctx: exprstatic::NewExprContext(Vec::new()),
    })
}

fn column(unique_id: i64) -> expression::Column {
    expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        unique_id,
        unique_id,
        0,
    )
}

#[test]
fn index_scan_reader_uses_data_source_schema_like_go() {
    let ctx = context();
    let mut scan = PhysicalIndexScan::New(ctx.clone());
    scan.PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![column(11)]));
    scan.DataSourceSchema = Some(expression::NewSchema(vec![column(22), column(33)]));

    let mut reader = PhysicalIndexReader::New(ctx);
    reader.SetChildren(vec![Box::new(scan)]);

    assert_eq!(reader.schema().Columns.len(), 2);
    assert_eq!(reader.schema().Columns[0].UniqueID, 22);
    assert_eq!(reader.OutputColumns[1].UniqueID, 33);
}

#[test]
fn projection_index_reader_keeps_projection_schema_like_go() {
    let ctx = context();
    let mut projection = PhysicalProjection::New(ctx.clone());
    projection
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![column(44)]));

    let mut reader = PhysicalIndexReader::New(ctx);
    reader.SetChildren(vec![Box::new(projection)]);

    assert_eq!(reader.schema().Columns[0].UniqueID, 44);
    assert_eq!(reader.OutputColumns[0].UniqueID, 44);
}

#[test]
fn resolve_indices_falls_back_to_matching_virtual_expression_like_go() {
    let ctx = context();
    let mut candidate = column(11);
    candidate.VirtualExpr = Some(Box::new(expression::NewOne()));
    let mut target = column(22);
    target.VirtualExpr = Some(Box::new(expression::NewOne()));

    let mut scan = PhysicalIndexScan::New(ctx.clone());
    scan.PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![candidate]));

    let mut reader = PhysicalIndexReader::New(ctx);
    reader.IndexPlan = Some(Box::new(scan));
    reader.OutputColumns = vec![target];

    reader.ResolveIndices().expect("resolve virtual index");
    assert_eq!(reader.OutputColumns[0].Index, 0);
}
