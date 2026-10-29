// Copyright 2026 AsterSQL.
use crate::Expression;
#[test]
fn pg08_06_formal() {
    let utc = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let cases = [
        ("UTC", "hour", Some("2024-01-02 01:30:00+00"), Some("1")),
        (
            "America/New_York",
            "hour",
            Some("2024-01-02 01:30:00+00"),
            Some("20"),
        ),
        (
            "America/New_York",
            "hour",
            Some("2024-11-03 05:30:00+00"),
            Some("1"),
        ),
        (
            "America/New_York",
            "hour",
            Some("2024-11-03 06:30:00+00"),
            Some("1"),
        ),
        (
            "America/New_York",
            "timezone",
            Some("2024-11-03 05:30:00+00"),
            Some("-14400"),
        ),
        (
            "America/New_York",
            "timezone",
            Some("2024-11-03 06:30:00+00"),
            Some("-18000"),
        ),
        (
            "Asia/Kathmandu",
            "timezone_hour",
            Some("2024-01-02 01:30:00+00"),
            Some("5"),
        ),
        (
            "Asia/Kathmandu",
            "timezone_minute",
            Some("2024-01-02 01:30:00+00"),
            Some("45"),
        ),
        (
            "America/New_York",
            "second",
            Some("2024-01-02 01:30:40.123456+00"),
            Some("40.123456"),
        ),
        (
            "America/New_York",
            "microseconds",
            Some("2024-01-02 01:30:40.123456+00"),
            Some("40123456"),
        ),
        (
            "America/New_York",
            "epoch",
            Some("2000-01-01 00:00:00+00"),
            Some("946684800.000000"),
        ),
        ("UTC", "hour", None, None),
        ("UTC", "timezone", Some("infinity"), None),
        ("UTC", "year", Some("infinity"), Some("Infinity")),
    ];
    for (index, (zone, unit, source, expected)) in cases.into_iter().enumerate() {
        let mut value = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        value.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypePgTimestampTz));
        let expression = crate::NewFunctionBase(
            &utc,
            "extract",
            *crate::types::NewFieldType(crate::mysql::TypeNewDecimal),
            vec![Box::new(crate::NewStrConst(unit)), Box::new(value)],
        )
        .unwrap();
        assert_eq!(
            expression.GetType(utc.GetEvalCtx()).GetType(),
            crate::mysql::TypeNewDecimal
        );
        let expression = crate::FoldConstant(&utc, expression);
        assert!(
            expression.as_scalar_function().is_some(),
            "stable extract folded"
        );
        let eval = exprstatic::NewEvalContext(vec![
            exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
            exprstatic::WithLocation(zone.parse().unwrap()),
        ]);
        let (actual, null) = expression
            .EvalString(&eval, crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!null).then_some(actual.as_str()), expected, "F-{index}");
    }
    let mut value = crate::NewStrConst("2024-01-02 01:30:00+00");
    value.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypePgTimestampTz));
    let expression = crate::NewFunctionBase(
        &utc,
        "extract",
        *crate::types::NewFieldType(crate::mysql::TypeNewDecimal),
        vec![
            Box::new(crate::NewStrConst("aster_bad_unit")),
            Box::new(value),
        ],
    )
    .unwrap();
    assert!(
        expression
            .EvalString(utc.GetEvalCtx(), crate::chunk::Row::default())
            .is_err()
    );
    println!("pg08_06_formal: 15 scenarios passed");
}
