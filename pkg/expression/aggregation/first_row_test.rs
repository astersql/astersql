// Copyright 2026 AsterSQL.

use crate::*;
use std::sync::Arc;

fn eval_context() -> Arc<dyn expression::EvalContext> {
    Arc::new(exprstatic::NewEvalContext(Vec::new()))
}

#[test]
fn reset_context_matches_go_and_preserves_accumulated_value() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let desc = NewAggFuncDesc(
        &expr_ctx,
        ast::AggFuncFirstRow,
        vec![Box::new(expression::Column::new(
            *types::NewFieldType(mysql::TypeLonglong),
            1,
            1,
            0,
        ))],
        false,
    )
    .unwrap();
    let mut first_row = desc.GetAggFunc(&expr_ctx);
    let mut state = first_row.CreateContext(eval_context());
    state.Value.SetInt64(42);
    state.GotFirstRow = true;

    first_row.ResetContext(eval_context(), &mut state);

    assert!(!state.GotFirstRow);
    assert!(!state.Value.IsNull());
    assert_eq!(state.Value.GetInt64(), 42);
}
