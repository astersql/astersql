// Copyright 2026 AsterSQL.
//! Numeric template formatting through the production scalar factory.
use crate::Expression;
#[test]
fn pg08_18_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (id, source, template, expected) in [
        ("main", Some("1234.5"), Some("FM9G999D0"), Some("1,234.5")),
        (
            "negative",
            Some("-148.5"),
            Some("FM999D990"),
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
            Some("\"x\"FM999D990"),
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
    for (numeric, monetary, source, template, wanted) in [
        (
            "de_DE.UTF-8",
            "C",
            Some("1234.5"),
            Some("FM9G999D0"),
            Some("1.234,5"),
        ),
        (
            "de_DE.UTF-8",
            "C",
            Some("1234.5"),
            Some("FM9,999.0"),
            Some("1,234.5"),
        ),
        (
            "C",
            "C",
            Some("1234.5"),
            Some("FML9G999D0"),
            Some(" 1,234.5"),
        ),
        (
            "C",
            "de_DE.UTF-8",
            Some("1234.5"),
            Some("FML9G999D0"),
            Some("€1,234.5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            Some("1234.5"),
            Some("FML9G999D0"),
            Some("€1.234,5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            Some("1234.5"),
            Some("FMS9G999D0"),
            Some("+1.234,5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            Some("-1234.5"),
            Some("FMS9G999D0"),
            Some("-1.234,5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            Some("-1234.5"),
            Some("FM9G999D0S"),
            Some("1.234,5-"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            Some("12.5"),
            Some("9G999D0"),
            Some("    12,5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            Some("0.1"),
            Some("FM9G999D0"),
            Some(",1"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            Some("0"),
            Some("FM9G999D0"),
            Some(",0"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            Some("12.5"),
            Some("\"D G S L\" FM999D0"),
            Some("D G S L 12,5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            None,
            Some("FML9G999D0S"),
            None,
        ),
        ("de_DE.UTF-8", "de_DE.UTF-8", Some("1234.5"), None, None),
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
        .unwrap();
        let expression = crate::FoldConstant(&context, expression);
        assert!(expression.as_scalar_function().is_some());
        let eval = exprstatic::NewEvalContext(vec![
            exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
            exprstatic::WithPgNumericLocales([numeric.into(), monetary.into()]),
        ]);
        let snapshot = exprstatic::MakeEvalContextStatic(&eval);
        let (actual, null) = expression
            .EvalString(&snapshot, crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!null).then_some(actual.as_str()), wanted);
    }
    println!("pg08_18_formal: 32 scenarios passed");
}
