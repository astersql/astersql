// Copyright 2026 AsterSQL.
//! Interval component formatting through the production scalar factory.
use crate::Expression;
#[test]
fn pg08_13_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (id, source, template, expected) in [
        (
            "hours",
            Some("36 hours"),
            Some("HH24:MI:SS"),
            Some("36:00:00"),
        ),
        (
            "negative",
            Some("-36 hours -2 minutes -3.123456 seconds"),
            Some("HH24:MI:SS.US"),
            Some("-36:-02:-03.-123456"),
        ),
        (
            "components",
            Some("14 months 3 days 36:02:03.123456"),
            Some("YYYY-MM-DD HH24:MI:SS.US"),
            Some("0001-02-03 36:02:03.123456"),
        ),
        (
            "negative-months",
            Some("-14 months 3 days -00:00:00.123456"),
            Some("YYYY-MM-DD HH24:MI:SS.US"),
            Some("-0001--02-03 00:00:00.-123456"),
        ),
        (
            "fill",
            Some("36:02:03.000004"),
            Some("FMHH24 FMMI FMSS FMUS"),
            Some("36 2 3 000004"),
        ),
        ("literal", Some("36 hours"), Some("hello!"), Some("hello!")),
        (
            "quoted",
            Some("36 hours"),
            Some("\"HH24\" HH24"),
            Some("HH24 36"),
        ),
        ("percent", Some("36 hours"), Some("%Y"), Some("%0")),
        ("empty", Some("36 hours"), Some(""), None),
        ("infinity", Some("infinity"), Some("YYYY"), None),
        ("negative-infinity", Some("-infinity"), Some("YYYY"), None),
        ("null", None, Some("HH24"), None),
        ("null-template", Some("36 hours"), None, None),
    ] {
        let mut source = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        source.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypePgInterval));
        let mut template = template
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        template.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeVarString));
        let expression = crate::NewFunctionBase(
            &context,
            "to_char",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![Box::new(source), Box::new(template)],
        )
        .unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(
            expression.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypeVarString
        );
        let (actual, null) = expression
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!null).then_some(actual.as_str()), expected, "{id}");
    }
    for template in ["TZH", "OF"] {
        let mut source = crate::NewStrConst("36 hours");
        source.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypePgInterval));
        let expression = crate::NewFunctionBase(
            &context,
            "to_char",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![Box::new(source), Box::new(crate::NewStrConst(template))],
        )
        .unwrap();
        let error = expression
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap_err();
        assert_eq!(
            error
                .into_shared()
                .downcast_ref::<types_dependency::pg_temporal::PgTemporalError>()
                .unwrap()
                .sqlstate(),
            "22007"
        );
    }
    println!("pg08_13_formal: 15 scenarios passed");
}
