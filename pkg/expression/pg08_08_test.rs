// Copyright 2026 AsterSQL.
//! date_part through the production PostgreSQL scalar factory.
use crate::Expression;
#[test]
fn pg08_08_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let cases = [
        (
            "F-second",
            "second",
            Some("2024-01-02 00:00:40.123456"),
            Some("40.123456"),
        ),
        (
            "F-microseconds",
            "microseconds",
            Some("2024-01-02 00:00:40.123456"),
            Some("40123456"),
        ),
        (
            "F-year",
            "year",
            Some("2024-01-02 00:00:40.123456"),
            Some("2024"),
        ),
        (
            "F-month",
            "month",
            Some("2024-01-02 00:00:40.123456"),
            Some("1"),
        ),
        (
            "F-day",
            "day",
            Some("2024-01-02 00:00:40.123456"),
            Some("2"),
        ),
        ("F-week", "week", Some("2021-01-01 12:34:56"), Some("53")),
        (
            "F-isoyear",
            "isoyear",
            Some("2021-01-01 12:34:56"),
            Some("2020"),
        ),
        ("F-null", "second", None, None),
        (
            "F-rounding",
            "second",
            Some("2000-01-01 00:00:01.003691"),
            Some("1.0036909999999999"),
        ),
        (
            "F-infinity-year",
            "year",
            Some("infinity"),
            Some("Infinity"),
        ),
        (
            "F-negative-infinity-year",
            "year",
            Some("-infinity"),
            Some("-Infinity"),
        ),
        ("F-infinity-second", "second", Some("infinity"), None),
        (
            "F-milliseconds",
            "milliseconds",
            Some("2024-01-02 00:00:40.123456"),
            Some("40123.456"),
        ),
        (
            "F-milliseconds-infinity",
            "milliseconds",
            Some("infinity"),
            None,
        ),
        (
            "F-minute",
            "minute",
            Some("2024-01-02 00:00:40.123456"),
            Some("0"),
        ),
        ("F-minute-infinity", "minute", Some("infinity"), None),
        (
            "F-hour",
            "hour",
            Some("2024-01-02 00:00:40.123456"),
            Some("0"),
        ),
        ("F-hour-infinity", "hour", Some("infinity"), None),
        (
            "F-quarter",
            "quarter",
            Some("2024-01-02 00:00:40.123456"),
            Some("1"),
        ),
        ("F-quarter-infinity", "quarter", Some("infinity"), None),
        (
            "F-decade",
            "decade",
            Some("2024-01-02 00:00:40.123456"),
            Some("202"),
        ),
        (
            "F-decade-infinity",
            "decade",
            Some("infinity"),
            Some("Infinity"),
        ),
        (
            "F-century",
            "century",
            Some("2024-01-02 00:00:40.123456"),
            Some("21"),
        ),
        (
            "F-century-infinity",
            "century",
            Some("infinity"),
            Some("Infinity"),
        ),
        (
            "F-millennium",
            "millennium",
            Some("2024-01-02 00:00:40.123456"),
            Some("3"),
        ),
        (
            "F-millennium-infinity",
            "millennium",
            Some("infinity"),
            Some("Infinity"),
        ),
        (
            "F-dow",
            "dow",
            Some("2024-01-02 00:00:40.123456"),
            Some("2"),
        ),
        ("F-dow-infinity", "dow", Some("infinity"), None),
        (
            "F-isodow",
            "isodow",
            Some("2024-01-02 00:00:40.123456"),
            Some("2"),
        ),
        ("F-isodow-infinity", "isodow", Some("infinity"), None),
        (
            "F-doy",
            "doy",
            Some("2024-01-02 00:00:40.123456"),
            Some("2"),
        ),
        ("F-doy-infinity", "doy", Some("infinity"), None),
        (
            "F-epoch",
            "epoch",
            Some("2024-01-02 00:00:40.123456"),
            Some("1704153640.123456"),
        ),
        (
            "F-epoch-infinity",
            "epoch",
            Some("infinity"),
            Some("Infinity"),
        ),
        (
            "F-julian",
            "julian",
            Some("2024-01-02 00:00:40.123456"),
            Some("2460312.000464392"),
        ),
        (
            "F-julian-infinity",
            "julian",
            Some("infinity"),
            Some("Infinity"),
        ),
        ("F-timezone-infinity", "timezone", Some("infinity"), None),
    ];
    for (id, unit, source, expected) in cases {
        let mut value = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        value.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeDatetime));
        let expression = crate::NewFunctionBase(
            &context,
            "date_part",
            *crate::types::NewFieldType(crate::mysql::TypeDouble),
            vec![Box::new(crate::NewStrConst(unit)), Box::new(value)],
        )
        .unwrap();
        assert_eq!(
            expression.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypeDouble,
            "{id}"
        );
        let (actual, null) = expression
            .EvalReal(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(
            (!null).then_some(actual),
            expected.map(|v| v.parse::<f64>().unwrap()),
            "{id}"
        );
    }
    for unit in ["timezone", "aster_bad_unit"] {
        let mut value = crate::NewStrConst("2024-01-02 00:00:40.123456");
        value.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeDatetime));
        let expression = crate::NewFunctionBase(
            &context,
            "date_part",
            *crate::types::NewFieldType(crate::mysql::TypeDouble),
            vec![Box::new(crate::NewStrConst(unit)), Box::new(value)],
        )
        .unwrap();
        assert!(
            expression
                .EvalReal(context.GetEvalCtx(), crate::chunk::Row::default())
                .is_err(),
            "{unit}"
        );
    }
    println!("pg08_08_formal: 39 scenarios passed");
}
