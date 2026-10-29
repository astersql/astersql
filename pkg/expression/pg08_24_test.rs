// Copyright 2026 AsterSQL.
use crate::Expression;
#[test]
fn pg08_24_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let cases = [
        (Some("1"), Some("3"), None, "[1,3)", ""),
        (Some("1"), Some("3"), Some(Some("[)")), "[1,3)", ""),
        (Some("1"), Some("3"), Some(Some("(]")), "(1,3]", ""),
        (Some("1"), Some("3"), Some(Some("[]")), "[1,3]", ""),
        (Some("1"), Some("3"), Some(Some("()")), "(1,3)", ""),
        (Some("4"), Some("4"), Some(Some("[]")), "[4,4]", ""),
        (Some("4"), Some("4"), Some(Some("[)")), "empty", ""),
        (Some("4"), Some("4"), Some(Some("(]")), "empty", ""),
        (Some("4"), Some("4"), Some(Some("()")), "empty", ""),
        (None, Some("4"), None, "(,4)", ""),
        (Some("1"), None, None, "[1,)", ""),
        (None, None, Some(Some("[]")), "(,)", ""),
        (
            Some("1.00000000000000000001"),
            Some("1.00000000000000000002"),
            Some(Some("(]")),
            "(1.00000000000000000001,1.00000000000000000002]",
            "",
        ),
        (Some("4"), Some("1"), None, "", "22000"),
        (Some("1"), Some("4"), Some(None), "", "22000"),
        (Some("1"), Some("4"), Some(Some("bad")), "", "42601"),
    ];
    for (a, b, flags, wanted, state) in cases {
        let mut args: Vec<Box<dyn Expression>> = vec![];
        for n in [a, b] {
            let mut v = n.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
            v.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeNewDecimal));
            args.push(Box::new(v));
        }
        if let Some(flags) = flags {
            let mut v = flags.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
            v.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeVarString));
            args.push(Box::new(v));
        }
        let expr = crate::NewFunctionBase(
            &context,
            "numrange",
            *crate::types::NewFieldType(crate::mysql::TypePgNumRange),
            args,
        )
        .expect("numrange formal factory");
        assert_eq!(
            expr.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypePgNumRange
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
                .parse::<types_dependency::pg_numrange::PgNumRange>()
                .unwrap();
            assert_eq!(actual.is_empty(), wanted == "empty");
            if wanted != "empty" {
                let (lo, hi) = wanted[1..wanted.len() - 1].split_once(',').unwrap();
                assert_eq!(
                    actual.lower().map(ToString::to_string),
                    (!lo.is_empty()).then(|| lo.to_owned())
                );
                assert_eq!(
                    actual.upper().map(ToString::to_string),
                    (!hi.is_empty()).then(|| hi.to_owned())
                );
                let raw = actual.to_binary();
                let flags = if lo.is_empty() {
                    8
                } else if wanted.starts_with('[') {
                    2
                } else {
                    0
                } | if hi.is_empty() {
                    16
                } else if wanted.ends_with(']') {
                    4
                } else {
                    0
                };
                assert_eq!(raw[0], flags);
                assert_eq!(
                    types_dependency::pg_numrange::PgNumRange::from_binary(&raw).unwrap(),
                    actual
                );
            }
        } else {
            let error = result.unwrap_err().into_shared();
            let diagnostic = error
                .downcast_ref::<types_dependency::pg_int4range::PgInt4RangeError>()
                .expect("range diagnostic");
            assert_eq!(diagnostic.state, state);
        }
    }
    println!("pg08_24_formal: 16 scenarios passed");
}
