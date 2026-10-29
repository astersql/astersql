// Copyright 2026 AsterSQL.
//! Explicit-zone truncation through the production scalar factory.
use crate::Expression;

#[test]
fn pg08_03_formal() {
    let cases = [
        (
            "UTC",
            Some("day"),
            Some("2024-01-02 01:30:00+00"),
            Some("2024-01-02 00:00:00+00"),
        ),
        (
            "America/New_York",
            Some("day"),
            Some("2024-01-02 01:30:00+00"),
            Some("2024-01-01 05:00:00+00"),
        ),
        (
            "America/New_York",
            Some("day"),
            Some("2024-03-10 16:00:00+00"),
            Some("2024-03-10 05:00:00+00"),
        ),
        (
            "America/New_York",
            Some("day"),
            Some("2024-03-11 16:00:00+00"),
            Some("2024-03-11 04:00:00+00"),
        ),
        (
            "America/New_York",
            Some("day"),
            Some("2024-11-03 16:00:00+00"),
            Some("2024-11-03 04:00:00+00"),
        ),
        (
            "America/New_York",
            Some("day"),
            Some("2024-11-04 16:00:00+00"),
            Some("2024-11-04 05:00:00+00"),
        ),
        ("UTC", Some("day"), None, None),
        ("UTC", None, Some("2024-01-02 01:30:00+00"), None),
    ];
    for (zone, unit, source, expected) in cases {
        let eval = exprstatic::NewEvalContext(vec![
            exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
            exprstatic::WithLocation("UTC".parse().unwrap()),
        ]);
        let context =
            exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(eval))]);
        let mut value = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        value.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypePgTimestampTz));
        let mut unit = unit.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
        unit.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeVarString));
        let expression = crate::NewFunctionBase(
            &context,
            "date_trunc",
            *crate::types::NewFieldType(crate::mysql::TypePgTimestampTz),
            vec![
                Box::new(unit),
                Box::new(value),
                Box::new(crate::NewStrConst(zone)),
            ],
        )
        .unwrap();
        assert_eq!(
            expression.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypePgTimestampTz
        );
        let (actual, null) = expression
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(
            (!null).then_some(actual.as_str()),
            expected,
            "{zone} {source:?}"
        );
    }
    let utc = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let ny = exprstatic::NewEvalContext(vec![
        exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
        exprstatic::WithLocation("America/New_York".parse().unwrap()),
    ]);
    let mut source = crate::NewStrConst("2024-01-02 01:30:00+00");
    source.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypePgTimestampTz));
    let expression = crate::NewFunctionBase(
        &utc,
        "date_trunc",
        *crate::types::NewFieldType(crate::mysql::TypePgTimestampTz),
        vec![
            Box::new(crate::NewStrConst("day")),
            Box::new(source.clone()),
            Box::new(crate::NewStrConst("America/New_York")),
        ],
    )
    .unwrap();
    let folded = crate::FoldConstant(&utc, expression);
    assert!(
        folded.as_scalar_function().is_some(),
        "stable timestamptz must survive plan-time folding"
    );
    assert_eq!(
        folded
            .EvalString(&ny, crate::chunk::Row::default())
            .unwrap(),
        ("2024-01-01 05:00:00+00".to_owned(), false)
    );
    for unit in ["nonsense", "timezone"] {
        let expression = crate::NewFunctionBase(
            &utc,
            "date_trunc",
            *crate::types::NewFieldType(crate::mysql::TypePgTimestampTz),
            vec![
                Box::new(crate::NewStrConst(unit)),
                Box::new(source.clone()),
                Box::new(crate::NewStrConst("UTC")),
            ],
        )
        .unwrap();
        assert!(
            expression
                .EvalString(utc.GetEvalCtx(), crate::chunk::Row::default())
                .is_err()
        );
    }
    for (id, zone, source_type, source, expected, error) in [
        (
            "F-null-zone",
            None,
            crate::mysql::TypePgTimestampTz,
            "2024-01-02 01:30:00+00",
            None,
            false,
        ),
        (
            "F-invalid-zone",
            Some("NoSuchZone"),
            crate::mysql::TypePgTimestampTz,
            "2024-01-02 01:30:00+00",
            None,
            true,
        ),
        (
            "F-timestamp-implicit",
            Some("UTC"),
            crate::mysql::TypeDatetime,
            "2024-01-02 01:30:00",
            Some("2024-01-02 00:00:00+00"),
            false,
        ),
        (
            "F-interval-rejected",
            Some("UTC"),
            crate::mysql::TypePgInterval,
            "1 day",
            None,
            true,
        ),
    ] {
        let mut source = crate::NewStrConst(source);
        source.RetType = Some(*crate::types::NewFieldType(source_type));
        let mut zone = zone.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
        zone.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeVarString));
        let built = crate::NewFunctionBase(
            &utc,
            "date_trunc",
            *crate::types::NewFieldType(crate::mysql::TypePgTimestampTz),
            vec![
                Box::new(crate::NewStrConst("day")),
                Box::new(source),
                Box::new(zone),
            ],
        );
        if source_type == crate::mysql::TypePgInterval {
            assert!(built.is_err(), "{id}");
            continue;
        }
        let actual = built
            .unwrap()
            .EvalString(utc.GetEvalCtx(), crate::chunk::Row::default());
        if error {
            assert!(actual.is_err(), "{id}");
        } else {
            let (value, null) = actual.unwrap();
            assert_eq!((!null).then_some(value.as_str()), expected, "{id}");
        }
    }
    println!("pg08_03_formal: 15 scenarios passed");
}
