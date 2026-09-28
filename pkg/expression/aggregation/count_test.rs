// Copyright 2026 AsterSQL.

use crate::*;
use std::sync::Arc;

#[test]
fn dedup_mode_does_not_produce_a_count() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let statement_ctx = stmtctx::NewStmtCtx();
    let mut desc = NewAggFuncDesc(
        &expr_ctx,
        ast::AggFuncCount,
        vec![Box::new(expression::Constant::with_type(
            types::NewIntDatum(1),
            *types::NewFieldType(mysql::TypeLonglong),
        ))],
        false,
    )
    .unwrap();
    desc.Mode = DedupMode;

    let mut count = desc.GetAggFunc(&expr_ctx);
    let mut state = count.CreateContext(Arc::new(exprstatic::NewEvalContext(Vec::new())));
    count
        .Update(
            &mut state,
            &statement_ctx,
            chunk::mutrow::MutRowFromDatums(Vec::new())
                .ToRow()
                .CopyConstruct(),
        )
        .unwrap();

    assert_eq!(count.GetResult(&state).GetInt64(), 0);
}
