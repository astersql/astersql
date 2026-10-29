// Copyright 2026 AsterSQL.
use crate::exprctx::EvalContext as _;
use crate::{BuildContext, Expression};

#[test]
fn pg08_29_formal() {
    fn context(database: &str) -> exprstatic::ExprContext {
        exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
            exprstatic::NewEvalContext(vec![
                exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
                exprstatic::WithPgCurrentSchema(
                    (!database.is_empty()).then(|| database.to_owned()),
                ),
            ]),
        ))])
    }
    let a = context("pg0828_a");
    let b = context("pg0828_b");
    let expr = crate::NewFunctionBase(
        &a,
        "current_schema",
        *crate::types::NewFieldType(crate::mysql::TypeVarString),
        vec![],
    )
    .expect("current_schema formal factory");
    assert_eq!(expr.GetType(a.GetEvalCtx()).GetType(), 0xdd);
    assert!(crate::function_traits_kernel::is_uncacheable(
        "current_schema"
    ));
    assert!(crate::function_traits_kernel::is_unfoldable(
        "current_schema"
    ));
    assert!(crate::function_traits_kernel::is_illegal_generated_column_function("current_schema"));
    let folded = crate::NewFunction(
        &a,
        "current_schema",
        *crate::types::NewFieldType(crate::mysql::TypeVarString),
        vec![],
    )
    .unwrap();
    assert!(folded.as_any().is::<crate::ScalarFunction>());
    let scalar = expr
        .as_any()
        .downcast_ref::<crate::ScalarFunction>()
        .unwrap();
    assert_eq!(scalar.Function.PbCode(), 0);
    assert!(!scalar.Function.SafeToShareAcrossSession());
    for ctx in [&a, &b, &a] {
        let expected = ctx.GetEvalCtx().PgCurrentSchema().unwrap();
        assert_eq!(
            folded
                .EvalString(ctx.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap(),
            (expected.clone(), false)
        );
        assert_eq!(
            expr.EvalString(ctx.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap(),
            (expected.clone(), false)
        );
        assert_eq!(
            expr.Eval(ctx.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap()
                .ToString()
                .unwrap(),
            expected
        );
    }
    let empty = context("");
    assert!(
        expr.EvalString(empty.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap()
            .1
    );
    assert!(
        expr.Eval(empty.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap()
            .IsNull()
    );
    assert!(
        crate::NewFunctionBase(
            &a,
            "current_schema",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![Box::new(crate::NewOne())]
        )
        .is_err()
    );
    let mysql = exprstatic::NewExprContext(vec![]);
    assert!(
        crate::NewFunctionBase(
            &mysql,
            "current_schema",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![]
        )
        .is_err()
    );
    println!("pg08_29_formal: 6 scenarios passed (A/B/A, arity, dialect)");
}
