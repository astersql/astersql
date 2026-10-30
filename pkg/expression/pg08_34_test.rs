// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};
#[test]
fn pg08_34_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for name in ["/", "div"] {
        for (left, right, wanted, error) in [
            (Some(5), Some(2), Some(2), None),
            (Some(-5), Some(2), Some(-2), None),
            (Some(5), Some(-2), Some(-2), None),
            (Some(-5), Some(-2), Some(2), None),
            (Some(i32::MIN), Some(1), Some(i32::MIN), None),
            (Some(i32::MAX), Some(1), Some(i32::MAX), None),
            (None, Some(0), None, None),
            (Some(1), None, None, None),
            (Some(i32::MIN), Some(-1), None, Some("integer out of range")),
            (Some(1), Some(0), None, Some("division by zero")),
        ] {
            let args = [left, right]
                .into_iter()
                .map(|v| {
                    let mut arg =
                        v.map_or_else(crate::NewNull, |n| crate::NewInt64Const(i64::from(n)));
                    arg.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeLong));
                    Box::new(arg) as Box<dyn Expression>
                })
                .collect();
            let f = crate::NewFunctionBase(
                &context,
                name,
                *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
                args,
            )
            .unwrap();
            assert_eq!(
                f.GetType(context.GetEvalCtx()).GetType(),
                crate::mysql::TypeLong
            );
            assert_eq!(f.as_scalar_function().unwrap().Function.PbCode(), 0);
            let actual = f.EvalInt(context.GetEvalCtx(), crate::chunk::Row::default());
            if let Some(message) = error {
                let e = actual.unwrap_err();
                assert_eq!(e.to_string(), message);
                let shared = e.into_shared();
                let e = shared.downcast_ref::<crate::PgInt4Error>().unwrap();
                assert_eq!(
                    e.sqlstate(),
                    if right == Some(0) { "22012" } else { "22003" }
                );
            } else {
                let (value, null) = actual.unwrap();
                assert_eq!((!null).then_some(value), wanted.map(i64::from));
            }
        }
    }
    let mysql = exprstatic::NewExprContext(Vec::new());
    let args = || {
        [5, 2]
            .into_iter()
            .map(|n| {
                let mut arg = crate::NewInt64Const(n);
                arg.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeLong));
                Box::new(arg) as Box<dyn Expression>
            })
            .collect()
    };
    let slash = crate::NewFunctionBase(
        &mysql,
        "/",
        *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
        args(),
    )
    .unwrap();
    assert_eq!(
        slash.GetType(mysql.GetEvalCtx()).GetType(),
        crate::mysql::TypeNewDecimal
    );
    let (value, null) = slash
        .EvalDecimal(mysql.GetEvalCtx(), crate::chunk::Row::default())
        .unwrap();
    assert!(!null);
    assert_eq!(
        std::str::from_utf8(&value.ToString())
            .unwrap()
            .parse::<f64>()
            .unwrap(),
        2.5
    );
    let div = crate::NewFunctionBase(
        &mysql,
        "intdiv",
        *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
        args(),
    )
    .unwrap();
    assert_eq!(
        div.EvalInt(mysql.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap(),
        (2, false)
    );
    println!("pg08_34_formal: 12 semantic cases; both PG factory spellings verified");
}
