// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::*;
use std::sync::Arc;

fn int_column(index: isize) -> expression::ExprBox {
    Box::new(expression::Column::new(
        *types::NewFieldType(mysql::TypeLonglong),
        index as i64 + 1,
        index as i64 + 1,
        index,
    ))
}

fn string_column(index: isize) -> expression::ExprBox {
    Box::new(expression::Column::new(
        *types::NewFieldType(mysql::TypeVarchar),
        index as i64 + 1,
        index as i64 + 1,
        index,
    ))
}

fn row(value: i64, separator: &str) -> chunk::Row {
    chunk::mutrow::MutRowFromDatums(vec![
        types::NewIntDatum(value),
        types::NewStringDatum(separator.to_owned()),
    ])
    .ToRow()
    .CopyConstruct()
}

fn eval_context() -> Arc<dyn expression::EvalContext> {
    Arc::new(exprstatic::NewEvalContext(Vec::new()))
}

#[test]
fn reset_preserves_lifetime_truncation_warning_like_go() {
    let expr_ctx = exprstatic::NewExprContext(vec![exprstatic::WithGroupConcatMaxLen(1)]);
    let desc = NewAggFuncDesc(
        &expr_ctx,
        ast::AggFuncGroupConcat,
        vec![int_column(0), string_column(1)],
        false,
    )
    .unwrap();
    let mut concat = desc.GetAggFunc(&expr_ctx);
    let statement_ctx = stmtctx::NewStmtCtx();
    let mut state = concat.CreateContext(eval_context());

    concat
        .Update(&mut state, &statement_ctx, row(12, "x"))
        .unwrap();
    assert_eq!(concat.GetResult(&state).GetString(), "1");
    assert_eq!(statement_ctx.GetWarnings().len(), 1);

    concat.ResetContext(eval_context(), &mut state);
    concat
        .Update(&mut state, &statement_ctx, row(34, "|"))
        .unwrap();
    concat
        .Update(&mut state, &statement_ctx, row(56, "|"))
        .unwrap();

    assert_eq!(concat.GetResult(&state).GetString(), "3");
    // Go/MySQL emits exactly one truncation warning for the aggregate lifetime.
    assert_eq!(statement_ctx.GetWarnings().len(), 1);
}

#[test]
fn reset_preserves_separator_initialized_for_the_aggregate_lifetime() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let desc = NewAggFuncDesc(
        &expr_ctx,
        ast::AggFuncGroupConcat,
        vec![int_column(0), string_column(1)],
        false,
    )
    .unwrap();
    let mut concat = desc.GetAggFunc(&expr_ctx);
    let statement_ctx = stmtctx::NewStmtCtx();
    let mut state = concat.CreateContext(eval_context());

    concat
        .Update(&mut state, &statement_ctx, row(1, "x"))
        .unwrap();
    concat.ResetContext(eval_context(), &mut state);
    concat
        .Update(&mut state, &statement_ctx, row(3, "|"))
        .unwrap();
    concat
        .Update(&mut state, &statement_ctx, row(4, "|"))
        .unwrap();

    assert_eq!(concat.GetResult(&state).GetString(), "3x4");
}
