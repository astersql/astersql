// Copyright 2026 AsterSQL.
//! Interval truncation through the production scalar factory.
use crate::Expression;
#[test]
fn pg08_04_formal() {
    use types_dependency::pg_temporal::PgInterval;
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let cases = [
        (
            "F-microseconds",
            Some("microseconds"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((27, 4, 18367123456)),
        ),
        (
            "F-milliseconds",
            Some("milliseconds"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((27, 4, 18367123000)),
        ),
        (
            "F-second",
            Some("second"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((27, 4, 18367000000)),
        ),
        (
            "F-minute",
            Some("minute"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((27, 4, 18360000000)),
        ),
        (
            "F-hour",
            Some("hour"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((27, 4, 18000000000)),
        ),
        (
            "F-day",
            Some("day"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((27, 4, 0)),
        ),
        (
            "F-month",
            Some("month"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((27, 0, 0)),
        ),
        (
            "F-quarter",
            Some("quarter"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((27, 0, 0)),
        ),
        (
            "F-year",
            Some("year"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((24, 0, 0)),
        ),
        (
            "F-decade",
            Some("decade"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((0, 0, 0)),
        ),
        (
            "F-century",
            Some("century"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((0, 0, 0)),
        ),
        (
            "F-millennium",
            Some("millennium"),
            Some("2 years 3 months 4 days 05:06:07.123456"),
            Some((0, 0, 0)),
        ),
        (
            "F-negative",
            Some("hour"),
            Some("-1 day -02:03:04"),
            Some((0, -1, -7200000000)),
        ),
        (
            "F-long-hour",
            Some("hour"),
            Some("27:06:07"),
            Some((0, 0, 97200000000)),
        ),
        (
            "F-negative-quarter",
            Some("quarter"),
            Some("-2 months"),
            Some((0, 0, 0)),
        ),
        (
            "F-negative-millisecond",
            Some("milliseconds"),
            Some("-00:00:00.123456"),
            Some((0, 0, -123000)),
        ),
        ("F-null-source", Some("hour"), None, None),
        ("F-null-unit", None, Some("1 day"), None),
    ];
    for (id, unit, source, expected) in cases {
        let mut value = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        value.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypePgInterval));
        let mut unit = unit.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
        unit.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeVarString));
        let expression = crate::NewFunctionBase(
            &context,
            "date_trunc",
            *crate::types::NewFieldType(crate::mysql::TypePgInterval),
            vec![Box::new(unit), Box::new(value)],
        )
        .unwrap();
        assert_eq!(
            expression.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypePgInterval,
            "{id}"
        );
        let (actual, null) = expression
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(
            (!null).then(|| actual.parse::<PgInterval>().unwrap().components()),
            expected,
            "{id}"
        );
    }
    for unit in ["week", "nonsense"] {
        let mut value = crate::NewStrConst("1 day");
        value.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypePgInterval));
        let expression = crate::NewFunctionBase(
            &context,
            "date_trunc",
            *crate::types::NewFieldType(crate::mysql::TypePgInterval),
            vec![Box::new(crate::NewStrConst(unit)), Box::new(value)],
        )
        .unwrap();
        assert!(
            expression
                .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
                .is_err()
        );
    }
    println!("pg08_04_formal: 20 scenarios passed");
}
