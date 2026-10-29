// Copyright 2026 AsterSQL.
//! EXTRACT through the production PostgreSQL scalar factory.
use crate::Expression;
#[test]
fn pg08_07_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let cases = [
        (
            "F-year",
            "year",
            Some("27 months 4 days 05:06:07.123456"),
            Some("2"),
        ),
        (
            "F-month",
            "month",
            Some("27 months 4 days 05:06:07.123456"),
            Some("3"),
        ),
        (
            "F-day",
            "day",
            Some("27 months 4 days 05:06:07.123456"),
            Some("4"),
        ),
        (
            "F-hour",
            "hour",
            Some("27 months 4 days 05:06:07.123456"),
            Some("5"),
        ),
        (
            "F-second",
            "second",
            Some("27 months 4 days 05:06:07.123456"),
            Some("7.123456"),
        ),
        ("F-minute", "minute", Some("80 minutes"), Some("20")),
        (
            "F-negative-year",
            "year",
            Some("-27 months -4 days -05:06:07.123456"),
            Some("-2"),
        ),
        (
            "F-negative-month",
            "month",
            Some("-27 months -4 days -05:06:07.123456"),
            Some("-3"),
        ),
        (
            "F-negative-day",
            "day",
            Some("-27 months -4 days -05:06:07.123456"),
            Some("-4"),
        ),
        (
            "F-negative-hour",
            "hour",
            Some("-27 months -4 days -05:06:07.123456"),
            Some("-5"),
        ),
        (
            "F-negative-second",
            "second",
            Some("-27 months -4 days -05:06:07.123456"),
            Some("-7.123456"),
        ),
        (
            "F-negative-fraction",
            "second",
            Some("-0.123456 seconds"),
            Some("-0.123456"),
        ),
        ("F-long-hour", "hour", Some("2 days 27 hours"), Some("27")),
        (
            "F-microseconds",
            "microseconds",
            Some("27 months 4 days 05:06:07.123456"),
            Some("7123456"),
        ),
        (
            "F-milliseconds",
            "milliseconds",
            Some("27 months 4 days 05:06:07.123456"),
            Some("7123.456"),
        ),
        ("F-null", "second", None, None),
    ];
    for (id, unit, source, expected) in cases {
        let mut value = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        value.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypePgInterval));
        let expression = crate::NewFunctionBase(
            &context,
            "extract",
            *crate::types::NewFieldType(crate::mysql::TypeNewDecimal),
            vec![Box::new(crate::NewStrConst(unit)), Box::new(value)],
        )
        .unwrap();
        assert_eq!(
            expression.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypeNewDecimal,
            "{id}"
        );
        let (actual, null) = expression
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!null).then_some(actual.as_str()), expected, "{id}");
    }
    for unit in ["timezone", "aster_bad_unit"] {
        let mut value = crate::NewStrConst("27 months 4 days 05:06:07.123456");
        value.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypePgInterval));
        let expression = crate::NewFunctionBase(
            &context,
            "extract",
            *crate::types::NewFieldType(crate::mysql::TypeNewDecimal),
            vec![Box::new(crate::NewStrConst(unit)), Box::new(value)],
        )
        .unwrap();
        assert!(
            expression
                .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
                .is_err(),
            "{unit}"
        );
    }
    println!("pg08_07_formal: 18 scenarios passed");
}
