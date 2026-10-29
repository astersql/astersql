// Copyright 2026 AsterSQL.
//! Numeric template formatting through the production scalar factory.
use crate::Expression;
#[test]
fn pg08_14_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (id, source, template, expected) in [
        ("main", Some("148.5"), Some("FM999.990"), Some("148.500")),
        (
            "negative",
            Some("-148.5"),
            Some("FM999.990"),
            Some("-148.500"),
        ),
        ("spaces", Some("12"), Some("9999"), Some("   12")),
        ("zero-fill", Some("12"), Some("0000"), Some(" 0012")),
        ("fraction", Some("0.1"), Some("FM9.990"), Some(".100")),
        ("optional", Some("12.3"), Some("FM99.999"), Some("12.3")),
        ("round", Some("9.995"), Some("FM99.00"), Some("10.00")),
        (
            "negative-round",
            Some("-9.995"),
            Some("FM99.00"),
            Some("-10.00"),
        ),
        ("overflow", Some("99.995"), Some("99.99"), Some(" ##.##")),
        ("zero", Some("0"), Some("999"), Some("   0")),
        ("fill-zero", Some("0"), Some("FM999"), Some("0")),
        (
            "literal-spaces",
            Some("12"),
            Some("  FM999  "),
            Some("  12  "),
        ),
        (
            "quoted",
            Some("148.5"),
            Some("\"x\"FM999.990"),
            Some("x148.500"),
        ),
        ("empty", Some("1"), Some(""), Some("")),
        ("null", None, Some("FM999"), None),
        ("null-template", Some("1"), None, None),
        (
            "exact",
            Some("9007199254740993"),
            Some("FM9999999999999999.0"),
            Some("9007199254740993.0"),
        ),
    ] {
        let mut source = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        source.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeNewDecimal));
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
    for template in ["99.9V99"] {
        let mut source = crate::NewStrConst("148.5");
        source.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeNewDecimal));
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
                .downcast_ref::<types_dependency::pg_numeric::PgNumericError>()
                .unwrap()
                .sqlstate(),
            "42601"
        );
    }
    println!("pg08_14_formal: 18 scenarios passed");
}
