// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};

#[test]
fn pg08_27_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let mut count = 0;
    for source in [crate::mysql::TypePgInt4Range, crate::mysql::TypePgNumRange] {
        for (input, expected) in [
            (Some("empty"), Some(1)),
            (None, None),
            (Some("(,)"), Some(0)),
            (Some("[2,5)"), Some(0)),
            (Some("[2,2]"), Some(0)),
            (Some("[2,2)"), Some(1)),
            (Some("(2,2]"), Some(1)),
            (Some("(2,2)"), Some(1)),
            (Some("(,5)"), Some(0)),
            (Some("[2,)"), Some(0)),
        ] {
            if let Some(text) = input {
                let (empty, flags) = if source == crate::mysql::TypePgInt4Range {
                    let r = text
                        .parse::<types_dependency::pg_int4range::PgInt4Range>()
                        .unwrap();
                    (r.is_empty(), r.to_binary()[0])
                } else {
                    let r = text
                        .parse::<types_dependency::pg_numrange::PgNumRange>()
                        .unwrap();
                    (r.is_empty(), r.to_binary()[0])
                };
                assert_eq!(empty, expected == Some(1));
                assert_eq!(flags & 1 != 0, empty);
                if text == "(,)" {
                    assert_eq!(flags, 24);
                }
            }
            let mut arg = input.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
            arg.RetType = Some(*crate::types::NewFieldType(source));
            let expr = crate::NewFunctionBase(
                &context,
                "isempty",
                *crate::types::NewFieldType(crate::mysql::TypeTiny),
                vec![Box::new(arg)],
            )
            .expect("isempty formal factory");
            let tp = expr.GetType(context.GetEvalCtx());
            assert_eq!(tp.GetType(), crate::mysql::TypeTiny);
            assert_ne!(tp.GetFlag() & crate::mysql::IsBooleanFlag, 0);
            let scalar = expr
                .as_any()
                .downcast_ref::<crate::ScalarFunction>()
                .unwrap();
            assert!(scalar.Function.SafeToShareAcrossSession());
            assert_eq!(scalar.Function.PbCode(), 0);
            let (v, n) = expr
                .EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap();
            assert_eq!((!n).then_some(v), expected);
            let datum = expr
                .Eval(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap();
            assert_eq!(datum.IsNull(), expected.is_none());
            if let Some(expected) = expected {
                assert_eq!(datum.ToString().unwrap(), expected.to_string());
            }
            count += 1;
        }
    }
    println!("pg08_27_formal: {count} scenarios passed");
}
