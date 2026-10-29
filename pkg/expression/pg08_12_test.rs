// Copyright 2026 AsterSQL.
//! Execution-zone formatting through the production scalar factory.
use crate::Expression;
#[test]
fn pg08_12_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (id, zone, source, template, expected) in [
        (
            "utc",
            "UTC",
            Some("2024-01-02 01:30:00+00"),
            Some("YYYY-MM-DD HH24:MI TZH:TZM"),
            Some("2024-01-02 01:30 +00:00"),
        ),
        (
            "ny",
            "America/New_York",
            Some("2024-01-02 01:30:00+00"),
            Some("YYYY-MM-DD HH24:MI TZH:TZM"),
            Some("2024-01-01 20:30 -05:00"),
        ),
        (
            "spring-before",
            "America/New_York",
            Some("2024-03-10 06:30:00+00"),
            Some("YYYY-MM-DD HH24:MI OF"),
            Some("2024-03-10 01:30 -05"),
        ),
        (
            "spring-after",
            "America/New_York",
            Some("2024-03-10 07:30:00+00"),
            Some("YYYY-MM-DD HH24:MI OF"),
            Some("2024-03-10 03:30 -04"),
        ),
        (
            "fall-before",
            "America/New_York",
            Some("2024-11-03 05:30:00+00"),
            Some("YYYY-MM-DD HH24:MI TZH:TZM"),
            Some("2024-11-03 01:30 -04:00"),
        ),
        (
            "fall-after",
            "America/New_York",
            Some("2024-11-03 06:30:00+00"),
            Some("YYYY-MM-DD HH24:MI TZH:TZM"),
            Some("2024-11-03 01:30 -05:00"),
        ),
        (
            "kathmandu",
            "Asia/Kathmandu",
            Some("2024-01-02 01:30:00+00"),
            Some("HH24:MI OF TZH:TZM FMTZH:FMTZM FMOF"),
            Some("07:15 +05:45 +05:45 +05:45 +5:45"),
        ),
        (
            "microseconds",
            "America/New_York",
            Some("2024-01-02 01:30:40.123456+00"),
            Some("HH24:MI:SS.US"),
            Some("20:30:40.123456"),
        ),
        (
            "empty",
            "UTC",
            Some("2024-01-02 01:30:00+00"),
            Some(""),
            None,
        ),
        (
            "infinity",
            "America/New_York",
            Some("infinity"),
            Some("YYYY OF"),
            None,
        ),
        (
            "negative-infinity",
            "America/New_York",
            Some("-infinity"),
            Some("YYYY OF"),
            None,
        ),
        ("null", "UTC", None, Some("YYYY OF"), None),
        (
            "null-template",
            "UTC",
            Some("2024-01-02 01:30:00+00"),
            None,
            None,
        ),
    ] {
        let mut source = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        source.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypePgTimestampTz));
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
        let folded = crate::FoldConstant(&context, expression);
        assert!(
            folded.as_scalar_function().is_some(),
            "{id}: stable timestamptz folded"
        );
        let eval = exprstatic::NewEvalContext(vec![
            exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
            exprstatic::WithLocation(zone.parse().unwrap()),
        ]);
        let (actual, null) = folded
            .EvalString(&eval, crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!null).then_some(actual.as_str()), expected, "{id}");
    }
    // A timestamp without time zone remains independent of the execution zone.
    for zone in ["UTC", "America/New_York"] {
        let mut source = crate::NewStrConst("2024-01-02 01:30:00");
        source.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeDatetime));
        let expression = crate::NewFunctionBase(
            &context,
            "to_char",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![
                Box::new(source),
                Box::new(crate::NewStrConst("YYYY-MM-DD HH24:MI")),
            ],
        )
        .unwrap();
        let eval = exprstatic::NewEvalContext(vec![
            exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
            exprstatic::WithLocation(zone.parse().unwrap()),
        ]);
        assert_eq!(
            expression
                .EvalString(&eval, crate::chunk::Row::default())
                .unwrap(),
            ("2024-01-02 01:30".to_owned(), false)
        );
    }
    assert!(
        crate::NewFunctionBase(
            &context,
            "to_char",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![
                Box::new(crate::NewStrConst("2024-01-02")),
                Box::new(crate::NewStrConst("YYYY"))
            ]
        )
        .is_err()
    );
    println!("pg08_12_formal: 16 scenarios passed");
}
