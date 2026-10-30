// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};
#[test]
fn pg08_38_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (left, right, equal) in [
        (None, None, true),
        (None, Some(1), false),
        (Some(1), None, false),
        (Some(1), Some(1), true),
        (Some(1), Some(2), false),
    ] {
        for (name, negate) in [("is not distinct from", false), ("is distinct from", true)] {
            let args = [left, right]
                .into_iter()
                .map(|v| {
                    let mut a = v.map(crate::NewInt64Const).unwrap_or_else(crate::NewNull);
                    a.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeLong));
                    Box::new(a) as Box<dyn Expression>
                })
                .collect();
            let f = crate::NewFunctionBase(
                &context,
                name,
                *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
                args,
            )
            .unwrap();
            let tp = f.GetType(context.GetEvalCtx());
            assert!(tp.GetFlag() & crate::mysql::IsBooleanFlag != 0);
            assert!(tp.GetFlag() & crate::mysql::NotNullFlag != 0);
            assert_eq!(
                f.EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
                    .unwrap(),
                (i64::from(equal ^ negate), false)
            );
            assert_eq!(f.as_scalar_function().unwrap().Function.PbCode(), 0);
        }
    }
    let f = crate::NewFunctionBase(
        &context,
        "eq",
        *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
        vec![
            Box::new(crate::NewNull()),
            Box::new(crate::NewInt64Const(1)),
        ],
    )
    .unwrap();
    assert!(
        f.EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap()
            .1
    );
    assert!(
        crate::NewFunctionBase(
            &context,
            "nulleq",
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            vec![Box::new(crate::NewNull()), Box::new(crate::NewNull())]
        )
        .is_err()
    );
    let mysql = exprstatic::NewExprContext(vec![]);
    let native = crate::NewFunctionBase(
        &mysql,
        "nulleq",
        *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
        vec![Box::new(crate::NewNull()), Box::new(crate::NewNull())],
    )
    .unwrap();
    let tp = native.GetType(mysql.GetEvalCtx());
    assert_eq!(tp.GetFlen(), 1);
    assert!(tp.GetFlag() & crate::mysql::BinaryFlag != 0);
    assert!(tp.GetFlag() & crate::mysql::NotNullFlag != 0);
    assert_eq!(
        native
            .EvalInt(mysql.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap(),
        (1, false)
    );
    println!("pg08_38_formal: 13 semantic cases");
}
