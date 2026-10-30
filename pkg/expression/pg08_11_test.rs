// Copyright 2026 AsterSQL.
//! Timestamp formatting through the production PostgreSQL scalar factory.
use crate::Expression;
#[test]
fn pg08_11_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (id, source, template, expected) in [
        (
            "microseconds",
            Some("2024-01-02 03:04:05.123456"),
            Some("YYYY-MM-DD HH24:MI:SS.US"),
            Some("2024-01-02 03:04:05.123456"),
        ),
        (
            "iso",
            Some("2021-01-01 00:00:00"),
            Some("IYYY-IW"),
            Some("2020-53"),
        ),
        (
            "modifiers",
            Some("2024-01-02 00:00:00"),
            Some("\"Year=\"YYYY FMDD DDTH"),
            Some("Year=2024 2 02ND"),
        ),
        (
            "percent",
            Some("2024-01-02 00:00:00"),
            Some("%Y"),
            Some("%4"),
        ),
        (
            "quoted",
            Some("2024-01-02 00:00:00"),
            Some("\"YYYY\""),
            Some("YYYY"),
        ),
        ("empty", Some("2024-01-02 00:00:00"), Some(""), None),
        ("infinity", Some("infinity"), Some("YYYY"), None),
        ("negative-infinity", Some("-infinity"), Some("YYYY"), None),
        ("null", None, Some("YYYY"), None),
        ("null-template", Some("2024-01-02 00:00:00"), None, None),
    ] {
        let mut source = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        source.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeDatetime));
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
        .unwrap_or_else(|error| panic!("{id}: {error}"));
        assert_eq!(
            expression.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypeVarString,
            "{id}"
        );
        let (actual, null) = expression
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!null).then_some(actual.as_str()), expected, "{id}");
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
    println!("pg08_11_formal: 11 scenarios passed");
}

#[test]
fn pg08_11_mysql_formal() {
    use crate::contextutil::WarnHandler;
    let warnings = std::sync::Arc::new(crate::contextutil::NewStaticWarnHandler(0));
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![
            exprstatic::WithSQLDialect(crate::ast::SqlDialect::MySQL),
            exprstatic::WithWarnHandler(warnings.clone()),
        ]),
    ))]);
    for (name, arguments, expected) in [
        (
            "date_format",
            vec![
                crate::NewStrConst("2024-01-02 03:04:05.123456"),
                crate::NewStrConst("%Y-%m-%d %H:%i:%s.%f"),
            ],
            Some("2024-01-02 03:04:05.123456"),
        ),
        (
            "date_format",
            vec![crate::NewStrConst("2024-01-02"), crate::NewStrConst("")],
            None,
        ),
        (
            "format",
            vec![
                crate::NewStrConst("12345.678"),
                crate::NewInt64Const(2),
                crate::NewStrConst("de_DE"),
            ],
            Some("12.345,68"),
        ),
        (
            "format",
            vec![crate::NewStrConst("12345.678"), crate::NewInt64Const(2)],
            Some("12,345.68"),
        ),
        (
            "format",
            vec![crate::NewNull(), crate::NewInt64Const(2)],
            None,
        ),
        (
            "format",
            vec![crate::NewStrConst("12345.678"), crate::NewNull()],
            None,
        ),
        (
            "format",
            vec![
                crate::NewStrConst("12345.678"),
                crate::NewInt64Const(2),
                crate::NewStrConst("aster_unknown"),
            ],
            Some("12,345.68"),
        ),
        (
            "format",
            vec![
                crate::NewStrConst("12345.678"),
                crate::NewInt64Const(2),
                crate::NewNull(),
            ],
            Some("12,345.68"),
        ),
    ] {
        let expression = crate::NewFunctionBase(
            &context,
            name,
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            arguments
                .into_iter()
                .map(|arg| Box::new(arg) as Box<dyn Expression>)
                .collect(),
        )
        .unwrap();
        let (actual, null) = expression
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!null).then_some(actual.as_str()), expected, "{name}");
        if name == "date_format" && expected.is_some() {
            let result = expression.GetType(context.GetEvalCtx());
            assert_eq!(result.GetFlen(), 31);
            assert!(!crate::mysql::HasNotNullFlag(result.GetFlag()));
            assert!(crate::mysql::HasBinaryFlag(result.GetFlag()));
        }
    }
    assert_eq!(warnings.CopyWarnings(Vec::new()).len(), 2);
    println!("pg08_11_mysql_formal: 8 scenarios passed; 2 locale warnings");
}
