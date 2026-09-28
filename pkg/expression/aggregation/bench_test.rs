// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// 聚合表达式相关基准测试。
//
// 以 AVG 为例，反复 CreateContext / ResetContext（含 DISTINCT），
// 用于对照 Go 侧聚合上下文创建开销；此处以功能断言代替真正计时。

use crate::*;
use std::sync::Arc;

/// 构造 longlong 列表达式，作为聚合参数。
fn int_column() -> expression::ExprBox {
    Box::new(expression::Column::new(
        *types::NewFieldType(mysql::TypeLonglong),
        1,
        1,
        0,
    ))
}

/// 循环创建或重置聚合求值上下文；空输入下结果应为 NULL。
fn exercise_context(has_distinct: bool, reset: bool) {
    let expression_context = exprstatic::NewExprContext(Vec::new());
    let descriptor = NewAggFuncDesc(
        &expression_context,
        ast::AggFuncAvg,
        vec![int_column()],
        has_distinct,
    )
    .unwrap();
    let mut aggregate = descriptor.GetAggFunc(&expression_context);
    let eval_context: Arc<dyn expression::EvalContext> =
        Arc::new(exprstatic::NewEvalContext(Vec::new()));
    let mut state = aggregate.CreateContext(eval_context.clone());
    for _ in 0..128 {
        if reset {
            aggregate.ResetContext(eval_context.clone(), &mut state);
        } else {
            state = aggregate.CreateContext(eval_context.clone());
        }
    }
    assert!(aggregate.GetResult(&state).IsNull());
}

/// 非 DISTINCT：反复 CreateContext。
#[test]
fn BenchmarkCreateContext() {
    exercise_context(false, false);
}

/// 非 DISTINCT：反复 ResetContext。
#[test]
fn BenchmarkResetContext() {
    exercise_context(false, true);
}

/// DISTINCT：反复 CreateContext。
#[test]
fn BenchmarkCreateDistinctContext() {
    exercise_context(true, false);
}

/// DISTINCT：反复 ResetContext。
#[test]
fn BenchmarkResetDistinctContext() {
    exercise_context(true, true);
}
