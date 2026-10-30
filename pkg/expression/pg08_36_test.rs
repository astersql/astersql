// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};
#[test]
fn pg08_36_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (left, right, wanted) in [
        (Some("5"), Some("2"), Some("2.5000000000000000")),
        (Some("-5"), Some("2"), Some("-2.5000000000000000")),
        (Some("1"), Some("3"), Some("0.33333333333333333333")),
        (Some("2"), Some("3"), Some("0.66666666666666666667")),
        (
            Some("9007199254740993"),
            Some("1"),
            Some("9007199254740993.0000"),
        ),
        (
            Some(
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
            ),
            Some("2"),
            Some("6172839450617283945061728394506172839450617283945061728394506172839450617283945"),
        ),
        (
            Some("0.00001"),
            Some("2"),
            Some("0.000005000000000000000000"),
        ),
        (
            Some("5.0000000000000000000000000"),
            Some("2"),
            Some("2.5000000000000000000000000"),
        ),
        (None, Some("0"), None),
        (Some("1"), None, None),
        (Some("NaN"), Some("0"), Some("NaN")),
        (Some("Infinity"), Some("-2"), Some("-Infinity")),
        (Some("5"), Some("Infinity"), Some("0")),
        (Some("Infinity"), Some("Infinity"), Some("NaN")),
    ] {
        let args = [left, right]
            .into_iter()
            .map(|v| {
                let mut a = v.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
                a.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeNewDecimal));
                Box::new(a) as Box<dyn Expression>
            })
            .collect();
        let f = crate::NewFunctionBase(
            &context,
            "/",
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            args,
        )
        .unwrap();
        assert_eq!(
            f.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypeNewDecimal
        );
        let (actual, null) = f
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(
            (!null).then_some(actual.as_str()),
            wanted,
            "{left:?}/{right:?}"
        );
        assert_eq!(f.as_scalar_function().unwrap().Function.PbCode(), 0);
    }
    let args = ["1", "0"]
        .into_iter()
        .map(|v| {
            let mut a = crate::NewStrConst(v);
            a.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeNewDecimal));
            Box::new(a) as Box<dyn Expression>
        })
        .collect();
    let f = crate::NewFunctionBase(
        &context,
        "/",
        *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
        args,
    )
    .unwrap();
    let e = f
        .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
        .unwrap_err();
    assert_eq!(e.to_string(), "division by zero");
    assert_eq!(
        e.into_shared()
            .downcast_ref::<types_dependency::pg_numeric::PgNumericError>()
            .unwrap()
            .sqlstate(),
        "22012"
    );
    println!("pg08_36_formal: 15 semantic cases");
}
