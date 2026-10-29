// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};
#[test]
fn pg08_30_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (source, input, oid) in [
        (crate::mysql::TypeLong, Some("1"), 23),
        (crate::mysql::TypeLong, None, 23),
        (crate::mysql::TypeVarString, None, 25),
        (crate::mysql::TypeVarString, Some("hello"), 25),
    ] {
        let mut arg = input.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
        arg.RetType = Some(*crate::types::NewFieldType(source));
        let expr = crate::NewFunctionBase(
            &context,
            "pg_typeof",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![Box::new(arg)],
        )
        .expect("pg_typeof formal factory");
        assert_eq!(expr.GetType(context.GetEvalCtx()).GetType(), 0xdc);
        assert_eq!(
            expr.EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap(),
            (oid.to_string(), false)
        );
        assert_eq!(
            expr.Eval(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap()
                .ToString()
                .unwrap(),
            oid.to_string()
        );
    }
    assert!(
        crate::NewFunctionBase(
            &context,
            "pg_typeof",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![]
        )
        .is_err()
    );
    let mysql = exprstatic::NewExprContext(vec![]);
    assert!(
        crate::NewFunctionBase(
            &mysql,
            "pg_typeof",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![Box::new(crate::NewOne())]
        )
        .is_err()
    );
    let failing = crate::NewFunctionBase(
        &context,
        "abs",
        *crate::types::NewFieldType(crate::mysql::TypeLonglong),
        vec![Box::new(crate::NewInt64Const(i64::MIN))],
    )
    .unwrap();
    let introspection = crate::NewFunctionBase(
        &context,
        "pg_typeof",
        *crate::types::NewFieldType(crate::mysql::TypeVarString),
        vec![failing],
    )
    .unwrap();
    assert!(
        introspection
            .Eval(context.GetEvalCtx(), crate::chunk::Row::default())
            .is_err(),
        "argument errors must survive type introspection"
    );
    println!("pg08_30_formal: 7 scenarios passed");
}
