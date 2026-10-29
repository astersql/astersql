// Copyright 2026 AsterSQL.
//! Floating point template formatting through the production scalar factory.
use crate::Expression;
#[test]
fn pg08_16_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (id, tp, source, template, expected) in [
        (
            "float4-basic",
            crate::mysql::TypeFloat,
            Some("125.5"),
            Some("FM999.0"),
            Some("125.5"),
        ),
        (
            "float4-tie",
            crate::mysql::TypeFloat,
            Some("2.5"),
            Some("FM9"),
            Some("2"),
        ),
        (
            "float4-precision",
            crate::mysql::TypeFloat,
            Some("1.23456789"),
            Some("FM9.999999999"),
            Some("1.23457"),
        ),
        (
            "float4-edge",
            crate::mysql::TypeFloat,
            Some("9.999999"),
            Some("FM99.999999999"),
            Some("10."),
        ),
        (
            "float4-nan",
            crate::mysql::TypeFloat,
            Some("NaN"),
            Some("FM999.0"),
            Some("NaN"),
        ),
        (
            "float4-inf",
            crate::mysql::TypeFloat,
            Some("Infinity"),
            Some("FM999.0"),
            Some("###."),
        ),
        (
            "float4-ninf",
            crate::mysql::TypeFloat,
            Some("-Infinity"),
            Some("FM999.0"),
            Some("-###."),
        ),
        (
            "float4-negzero",
            crate::mysql::TypeFloat,
            Some("-0"),
            Some("FM999.0"),
            Some("-.0"),
        ),
        (
            "float4-empty",
            crate::mysql::TypeFloat,
            Some("1"),
            Some(""),
            Some(""),
        ),
        (
            "float4-null",
            crate::mysql::TypeFloat,
            None,
            Some("FM999.0"),
            None,
        ),
        (
            "float4-null-template",
            crate::mysql::TypeFloat,
            Some("1"),
            None,
            None,
        ),
        (
            "float4-shift",
            crate::mysql::TypeFloat,
            Some("1.23456789"),
            Some("FM99V99"),
            Some("123"),
        ),
        (
            "float8-basic",
            crate::mysql::TypeDouble,
            Some("125.5"),
            Some("FM999.0"),
            Some("125.5"),
        ),
        (
            "float8-tie",
            crate::mysql::TypeDouble,
            Some("2.5"),
            Some("FM9"),
            Some("2"),
        ),
        (
            "float8-precision",
            crate::mysql::TypeDouble,
            Some("1.23456789"),
            Some("FM9.999999999"),
            Some("1.23456789"),
        ),
        (
            "float8-edge",
            crate::mysql::TypeDouble,
            Some("9.999999"),
            Some("FM99.999999999"),
            Some("9.999999"),
        ),
        (
            "float8-nan",
            crate::mysql::TypeDouble,
            Some("NaN"),
            Some("FM999.0"),
            Some("NaN"),
        ),
        (
            "float8-inf",
            crate::mysql::TypeDouble,
            Some("Infinity"),
            Some("FM999.0"),
            Some("###.#"),
        ),
        (
            "float8-ninf",
            crate::mysql::TypeDouble,
            Some("-Infinity"),
            Some("FM999.0"),
            Some("-###.#"),
        ),
        (
            "float8-negzero",
            crate::mysql::TypeDouble,
            Some("-0"),
            Some("FM999.0"),
            Some("-.0"),
        ),
        (
            "float8-empty",
            crate::mysql::TypeDouble,
            Some("1"),
            Some(""),
            Some(""),
        ),
        (
            "float8-null",
            crate::mysql::TypeDouble,
            None,
            Some("FM999.0"),
            None,
        ),
        (
            "float8-null-template",
            crate::mysql::TypeDouble,
            Some("1"),
            None,
            None,
        ),
        (
            "float8-shift",
            crate::mysql::TypeDouble,
            Some("1.23456789"),
            Some("FM99V99"),
            Some("123"),
        ),
    ] {
        let mut source = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        source.RetType = Some(*crate::types::NewFieldType(tp));
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
    for tp in [crate::mysql::TypeFloat, crate::mysql::TypeDouble] {
        let template = "99.9V99";
        let mut source = crate::NewInt64Const(1);
        source.RetType = Some(*crate::types::NewFieldType(tp));
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
    println!("pg08_16_formal: 26 scenarios passed");
}
