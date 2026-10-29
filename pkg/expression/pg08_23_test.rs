// Copyright 2026 AsterSQL.
use crate::Expression;
#[test]
fn pg08_23_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let cases = [
        (Some(4), Some(4), None, "empty", ""),
        (Some(4), Some(4), Some(Some("[]")), "[4,5)", ""),
        (Some(1), Some(4), Some(Some("(]")), "[2,5)", ""),
        (None, Some(4), None, "(,4)", ""),
        (Some(1), None, None, "[1,)", ""),
        (None, None, Some(Some("[]")), "(,)", ""),
        (Some(4), Some(1), None, "", "22000"),
        (Some(1), Some(4), Some(None), "", "22000"),
        (Some(1), Some(4), Some(Some("bad")), "", "42601"),
        (Some(1), Some(i32::MAX), Some(Some("[]")), "", "22003"),
    ];
    for (a, b, flags, wanted, state) in cases {
        let mut args: Vec<Box<dyn Expression>> = vec![];
        for n in [a, b] {
            let mut v = n
                .map(|n| crate::NewInt64Const(n as i64))
                .unwrap_or_else(crate::NewNull);
            v.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeLong));
            args.push(Box::new(v));
        }
        if let Some(flags) = flags {
            let mut v = flags.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
            v.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeVarString));
            args.push(Box::new(v));
        }
        let expr = crate::NewFunctionBase(
            &context,
            "int4range",
            *crate::types::NewFieldType(crate::mysql::TypePgInt4Range),
            args,
        )
        .expect("int4range formal factory");
        assert_eq!(
            expr.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypePgInt4Range
        );
        let scalar = expr
            .as_any()
            .downcast_ref::<crate::ScalarFunction>()
            .unwrap();
        assert!(scalar.Function.SafeToShareAcrossSession());
        assert_eq!(scalar.Function.PbCode(), 0);
        let result = expr.EvalString(context.GetEvalCtx(), crate::chunk::Row::default());
        if state.is_empty() {
            let (value, null) = result.unwrap();
            assert!(!null);
            assert_eq!(value, wanted);
            let actual = value
                .parse::<types_dependency::pg_int4range::PgInt4Range>()
                .unwrap();
            let expected = wanted
                .parse::<types_dependency::pg_int4range::PgInt4Range>()
                .unwrap();
            assert_eq!(actual.is_empty(), expected.is_empty());
            assert_eq!(actual.lower(), expected.lower());
            assert_eq!(actual.upper(), expected.upper());
            assert_eq!(
                types_dependency::pg_int4range::PgInt4Range::from_binary(&actual.to_binary())
                    .unwrap(),
                actual
            );
        } else {
            let error = result.unwrap_err().into_shared();
            let diagnostic = error
                .downcast_ref::<types_dependency::pg_int4range::PgInt4RangeError>()
                .expect("range diagnostic");
            assert_eq!(diagnostic.state, state);
        }
    }
    println!("pg08_23_formal: 10 scenarios passed");
}
