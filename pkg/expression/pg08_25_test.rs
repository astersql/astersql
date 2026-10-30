// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};

#[test]
fn pg08_25_formal() {
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
                (Some("[2,5)"), Some("2")),
                (Some("empty"), None),
                (Some("(,5)"), None),
                (Some("[2,)"), Some("2")),
                (Some("(,)"), None),
                (None, None),
            ],
        ),
        (
            crate::mysql::TypePgNumRange,
            crate::mysql::TypeNewDecimal,
            vec![
                (Some("(1,4]"), Some("1")),
                (Some("empty"), None),
                (Some("(,4]"), None),
                (
                    Some("[1.00000000000000000001,)"),
                    Some("1.00000000000000000001"),
                ),
                (Some("[1.00,4]"), Some("1.00")),
                (Some("(,)"), None),
                (None, None),
            ],
        ),
    ] {
        for (input, expected) in cases {
            let mut arg = input.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
            arg.RetType = Some(*crate::types::NewFieldType(source));
            let expr = crate::NewFunctionBase(
                &context,
                "lower",
                *crate::types::NewFieldType(result),
                vec![Box::new(arg)],
            )
            .expect("typed range lower formal factory");
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
            "lower",
            &[*crate::types::NewFieldType(source)],
        )
        .unwrap();
        assert_eq!(candidate.result, crate::mysql::TypeVarString);
    }
    println!("pg08_25_formal: {count} scenarios passed");
}

#[test]
fn pg08_25_mysql_case_factory() {
    let context = exprstatic::NewExprContext(vec![]);
    let mut count = 0;
    for (name, expected) in [("lower", "abc"), ("upper", "ABC")] {
        for (binary, null, flagged) in [
            (false, false, true),
            (true, false, true),
            (false, true, true),
            (false, false, false),
        ] {
            let mut argument = if null {
                crate::NewNull()
            } else {
                crate::NewStrConst("AbC")
            };
            let mut tp = *crate::types::NewFieldType(crate::mysql::TypeVarString);
            tp.SetFlen(3);
            tp.SetCharset(if binary { "binary" } else { "utf8mb4" }.into());
            tp.SetCollate(if binary { "binary" } else { "utf8mb4_bin" }.into());
            if flagged {
                tp.AddFlag(crate::mysql::BinaryFlag);
            }
            argument.RetType = Some(tp);
            let expression = crate::NewFunctionBase(
                &context,
                name,
                *crate::types::NewFieldType(crate::mysql::TypeVarString),
                vec![Box::new(argument)],
            )
            .expect("MySQL case conversion formal factory");
            let tp = expression.GetType(context.GetEvalCtx());
            assert_eq!(tp.GetFlen(), 3);
            assert_eq!(tp.GetCharset(), if binary { "binary" } else { "utf8mb4" });
            assert_eq!(
                tp.GetCollate(),
                if binary { "binary" } else { "utf8mb4_bin" }
            );
            assert_eq!(tp.GetFlag(), crate::mysql::BinaryFlag);
            let (value, is_null) = expression
                .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap();
            assert_eq!(is_null, null);
            if !null {
                assert_eq!(value, if binary { "AbC" } else { expected });
            }
            count += 1;
        }
    }
    println!("pg08_25_mysql_case_factory: {count} cases passed");
}
