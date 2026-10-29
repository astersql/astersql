// Copyright 2026 AsterSQL.
//! Timestamp truncation through the production scalar factory.
use crate::Expression;

#[test]
fn pg08_01_formal() {
    let eval = exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
        crate::ast::SqlDialect::PostgreSQL,
    )]);
    let context =
        exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(eval))]);
    let input = "2024-01-02 00:00:40.123456";
    let cases = [
        ("microseconds", Some(input), Some(input)),
        ("milliseconds", Some(input), Some("2024-01-02 00:00:40.123")),
        ("second", Some(input), Some("2024-01-02 00:00:40")),
        ("minute", Some(input), Some("2024-01-02 00:00:00")),
        ("hour", Some(input), Some("2024-01-02 00:00:00")),
        ("day", Some(input), Some("2024-01-02 00:00:00")),
        ("month", Some(input), Some("2024-01-01 00:00:00")),
        ("quarter", Some(input), Some("2024-01-01 00:00:00")),
        ("year", Some(input), Some("2024-01-01 00:00:00")),
        ("decade", Some(input), Some("2020-01-01 00:00:00")),
        ("century", Some(input), Some("2001-01-01 00:00:00")),
        ("millennium", Some(input), Some("2001-01-01 00:00:00")),
        (
            "week",
            Some("2021-01-01 12:34:56"),
            Some("2020-12-28 00:00:00"),
        ),
        (
            "day",
            Some("1969-12-31 23:59:59.999999"),
            Some("1969-12-31 00:00:00"),
        ),
        ("day", Some("infinity"), Some("infinity")),
        ("day", None, None),
    ];
    for (unit, value, expected) in cases {
        let mut timestamp = value.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
        timestamp.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeDatetime));
        let expression = crate::NewFunctionBase(
            &context,
            "date_trunc",
            *crate::types::NewFieldType(crate::mysql::TypeDatetime),
            vec![Box::new(crate::NewStrConst(unit)), Box::new(timestamp)],
        )
        .unwrap();
        assert_eq!(
            expression.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypeDatetime
        );
        let (actual, null) = expression
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(
            (!null).then_some(actual.as_str()),
            expected,
            "{unit}: {value:?}"
        );
    }
    for unit in ["nonsense", "timezone"] {
        let mut timestamp = crate::NewStrConst(input);
        timestamp.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeDatetime));
        let expression = crate::NewFunctionBase(
            &context,
            "date_trunc",
            *crate::types::NewFieldType(crate::mysql::TypeDatetime),
            vec![Box::new(crate::NewStrConst(unit)), Box::new(timestamp)],
        )
        .unwrap();
        assert!(
            expression
                .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
                .is_err(),
            "{unit}"
        );
    }
    let mysql_context = exprstatic::NewExprContext(Vec::new());
    for (unit, width) in [("second", 3), ("microsecond", 7), ("hour", 4)] {
        let expression = crate::NewFunctionBase(
            &mysql_context,
            "extract",
            *crate::types::NewFieldType(crate::mysql::TypeLonglong),
            vec![
                Box::new(crate::NewStrConst(unit)),
                Box::new(crate::NewStrConst("27:01:02.123456")),
            ],
        )
        .unwrap();
        let tp = expression.GetType(mysql_context.GetEvalCtx());
        assert_eq!(tp.GetType(), crate::mysql::TypeLonglong);
        assert_eq!(tp.GetFlen(), width, "{unit} native MySQL display width");
        assert_eq!(tp.GetDecimal(), 0);
        assert_ne!(tp.GetFlag() & crate::mysql::BinaryFlag, 0);
        assert_eq!(tp.GetFlag() & crate::mysql::NotNullFlag, 0);
        let expected = match unit {
            "second" => 2,
            "microsecond" => 123456,
            "hour" => 27,
            _ => unreachable!(),
        };
        assert_eq!(
            expression
                .EvalInt(mysql_context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap(),
            (expected, false),
            "{unit} preserves fractional TIME"
        );
    }
    println!("pg08_01_formal: 21 scenarios passed");
}
