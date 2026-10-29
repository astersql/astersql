// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};

#[test]
fn pg08_26_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let mut count = 0;
    for (source, result, cases) in [
        (
            crate::mysql::TypePgInt4Range,
            crate::mysql::TypeLong,
            vec![
                (Some("[2,5)"), Some("5")),
                (Some("empty"), None),
                (Some("(,5)"), Some("5")),
                (Some("[2,)"), None),
                (Some("(,)"), None),
                (None, None),
            ],
        ),
        (
            crate::mysql::TypePgNumRange,
            crate::mysql::TypeNewDecimal,
            vec![
                (Some("(1,4]"), Some("4")),
                (Some("empty"), None),
                (Some("(,4]"), Some("4")),
                (
                    Some("(,4.00000000000000000001]"),
                    Some("4.00000000000000000001"),
                ),
                (Some("[1,4.00]"), Some("4.00")),
                (Some("(,)"), None),
                (None, None),
            ],
        ),
    ] {
        for (input, expected) in cases {
            if let Some(text) = input {
                // Check empty/infinite bounds in the canonical carrier as well as
                // the scalar result; a display string alone cannot encode NULL.
                let (empty, lower_missing, upper_missing, flags) =
                    if source == crate::mysql::TypePgInt4Range {
                        let range = text
                            .parse::<types_dependency::pg_int4range::PgInt4Range>()
                            .unwrap();
                        (
                            range.is_empty(),
                            range.lower().is_none(),
                            range.upper().is_none(),
                            range.to_binary()[0],
                        )
                    } else {
                        let range = text
                            .parse::<types_dependency::pg_numrange::PgNumRange>()
                            .unwrap();
                        (
                            range.is_empty(),
                            range.lower().is_none(),
                            range.upper().is_none(),
                            range.to_binary()[0],
                        )
                    };
                assert_eq!(empty, text == "empty");
                assert_eq!(upper_missing, expected.is_none());
                assert_eq!(lower_missing, empty || text.starts_with("(,"));
                let expected_flags = if empty {
                    1
                } else {
                    u8::from(text.starts_with('[')) * 2
                        | u8::from(text.ends_with(']')) * 4
                        | u8::from(lower_missing) * 8
                        | u8::from(upper_missing) * 16
                };
                assert_eq!(flags, expected_flags);
            }
            let mut arg = input.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
            arg.RetType = Some(*crate::types::NewFieldType(source));
            let expr = crate::NewFunctionBase(
                &context,
                "upper",
                *crate::types::NewFieldType(result),
                vec![Box::new(arg)],
            )
            .expect("typed range upper formal factory");
            assert_eq!(expr.GetType(context.GetEvalCtx()).GetType(), result);
            let scalar = expr
                .as_any()
                .downcast_ref::<crate::ScalarFunction>()
                .unwrap();
            assert!(scalar.Function.SafeToShareAcrossSession());
            assert_eq!(scalar.Function.PbCode(), 0);
            let (v, null) = if source == crate::mysql::TypePgInt4Range {
                let (v, n) = expr
                    .EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
                    .unwrap();
                (v.to_string(), n)
            } else {
                expr.EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
                    .unwrap()
            };
            assert_eq!((!null).then_some(v).as_deref(), expected);
            let datum = expr
                .Eval(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap();
            assert_eq!(datum.IsNull(), expected.is_none());
            if let Some(expected) = expected {
                assert_eq!(datum.ToString().unwrap(), expected);
            }
            count += 1;
        }
    }
    // Adding range candidates must preserve unknown/string overload selection.
    for source in [
        crate::mysql::TypeNull,
        crate::mysql::TypeVarString,
        crate::mysql::TypeString,
    ] {
        let candidate = crate::formal_registry::resolve_pg_typed_signature(
            "upper",
            &[*crate::types::NewFieldType(source)],
        )
        .unwrap();
        assert_eq!(candidate.result, crate::mysql::TypeVarString);
    }
    println!("pg08_26_formal: {count} scenarios passed");
}
