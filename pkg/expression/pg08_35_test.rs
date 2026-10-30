// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};
#[test]
fn pg08_35_formal() {
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for name in ["-", "unaryminus"] {
        for (input, expected) in [
            (Some(5), Some(-5)),
            (Some(-5), Some(5)),
            (Some(0), Some(0)),
            (Some(i32::MAX), Some(-i32::MAX)),
            (Some(-i32::MAX), Some(i32::MAX)),
            (None, None),
            (Some(i32::MIN), None),
        ] {
            let mut arg = input.map_or_else(crate::NewNull, |n| crate::NewInt64Const(i64::from(n)));
            arg.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeLong));
            let f = crate::NewFunctionBase(
                &pg,
                name,
                *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
                vec![Box::new(arg)],
            )
            .unwrap();
            assert_eq!(f.GetType(pg.GetEvalCtx()).GetType(), crate::mysql::TypeLong);
            assert_eq!(f.as_scalar_function().unwrap().Function.PbCode(), 0);
            let actual = f.EvalInt(pg.GetEvalCtx(), crate::chunk::Row::default());
            if input == Some(i32::MIN) {
                let error = actual.unwrap_err().into_shared();
                let error = error.downcast_ref::<crate::PgInt4Error>().unwrap();
                assert_eq!(error.sqlstate(), "22003");
                assert_eq!(error.to_string(), "integer out of range");
            } else {
                let (value, null) = actual.unwrap();
                assert_eq!((!null).then_some(value), expected.map(i64::from));
            }
        }
    }
    let mysql = exprstatic::NewExprContext(Vec::new());
    for (input, expected) in [
        (Some(5), Some(-5)),
        (Some(-2147483648), Some(2147483648)),
        (None, None),
        (Some(i64::MAX), Some(-i64::MAX)),
        (Some(i64::MIN), None),
    ] {
        let mut arg = input.map_or_else(crate::NewNull, crate::NewInt64Const);
        arg.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeLonglong));
        let f = crate::NewFunctionBase(
            &mysql,
            "unaryminus",
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            vec![Box::new(arg)],
        )
        .unwrap();
        let actual = f.EvalInt(mysql.GetEvalCtx(), crate::chunk::Row::default());
        if input == Some(i64::MIN) {
            assert!(actual.is_err());
        } else {
            let (value, null) = actual.unwrap();
            assert_eq!((!null).then_some(value), expected);
        }
    }
    println!("pg08_35_formal: 12 semantic cases (two PG factory spellings)");
}
