// Copyright 2026 AsterSQL.

use crate::*;

#[test]
fn reset_context_matches_go_and_preserves_unrelated_state() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let desc = NewAggFuncDesc(
        &expr_ctx,
        ast::AggFuncBitXor,
        vec![Box::new(expression::Column::new(
            *types::NewFieldType(mysql::TypeLonglong),
            1,
            1,
            0,
        ))],
        false,
    )
    .unwrap();
    let mut bit_xor = desc.GetAggFunc(&expr_ctx);
    let eval_ctx = std::sync::Arc::new(exprstatic::NewEvalContext(Vec::new()));
    let mut state = bit_xor.CreateContext(eval_ctx.clone());
    state.Count = 7;
    state.Buffer = vec![1, 2, 3];
    state.BufferInitialized = true;
    state.GotFirstRow = true;
    state.Value.SetUint64(42);

    bit_xor.ResetContext(eval_ctx, &mut state);

    assert_eq!(state.Value.GetUint64(), 0);
    assert_eq!(state.Count, 7);
    assert_eq!(state.Buffer, vec![1, 2, 3]);
    assert!(state.BufferInitialized);
    assert!(state.GotFirstRow);
}
