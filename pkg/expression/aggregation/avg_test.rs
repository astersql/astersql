// Copyright 2026 AsterSQL.

use crate::*;

#[test]
fn zero_count_float_partial_state_matches_go_ieee_division() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let desc = NewAggFuncDesc(
        &expr_ctx,
        ast::AggFuncAvg,
        vec![Box::new(expression::Column::new(
            *types::NewFieldType(mysql::TypeDouble),
            1,
            1,
            0,
        ))],
        false,
    )
    .unwrap();
    let avg = desc.GetAggFunc(&expr_ctx);
    let mut state = avg.CreateContext(std::sync::Arc::new(exprstatic::NewEvalContext(Vec::new())));
    state.Value = types::NewFloat64Datum(1.0);
    state.Count = 0;

    assert!(avg.GetResult(&state).GetFloat64().is_infinite());
}

#[test]
fn zero_count_decimal_partial_state_matches_go_logged_error_result() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let desc = NewAggFuncDesc(
        &expr_ctx,
        ast::AggFuncAvg,
        vec![Box::new(expression::Column::new(
            *types::NewFieldType(mysql::TypeNewDecimal),
            1,
            1,
            0,
        ))],
        false,
    )
    .unwrap();
    let avg = desc.GetAggFunc(&expr_ctx);
    let mut state = avg.CreateContext(std::sync::Arc::new(exprstatic::NewEvalContext(Vec::new())));
    state.Value = types::NewDecimalDatum(types::NewDecFromInt(1));
    state.Count = 0;

    let result = avg.GetResult(&state);
    assert!(!result.IsNull());
    assert!(result.GetMysqlDecimal().IsZero());
}
