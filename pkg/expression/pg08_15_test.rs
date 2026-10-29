// Copyright 2026 AsterSQL.
//! Integer template formatting through the production scalar factory.
use crate::Expression;
#[test]
fn pg08_15_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (id, tp, source, template, expected) in [
        (
            "minus",
            crate::mysql::TypeLong,
            Some(-485i64),
            Some("999MI"),
            Some("485-"),
        ),
        (
            "positive-mi",
            crate::mysql::TypeLong,
            Some(485i64),
            Some("999MI"),
            Some("485 "),
        ),
        (
            "zero-fill",
            crate::mysql::TypeLonglong,
            Some(12i64),
            Some("FM000"),
            Some("012"),
        ),
        (
            "i4-min",
            crate::mysql::TypeLong,
            Some(-2147483648i64),
            Some("FM9999999999"),
            Some("-2147483648"),
        ),
        (
            "i4-max",
            crate::mysql::TypeLong,
            Some(2147483647i64),
            Some("FM9999999999"),
            Some("2147483647"),
        ),
        (
            "i8-min",
            crate::mysql::TypeLonglong,
            Some(-9223372036854775808i64),
            Some("FM9999999999999999999"),
            Some("-9223372036854775808"),
        ),
        (
            "i8-max",
            crate::mysql::TypeLonglong,
            Some(9223372036854775807i64),
            Some("FM9999999999999999999"),
            Some("9223372036854775807"),
        ),
        (
            "exact",
            crate::mysql::TypeLonglong,
            Some(9007199254740993i64),
            Some("FM9999999999999999"),
            Some("9007199254740993"),
        ),
        ("i4-null", crate::mysql::TypeLong, None, Some("999MI"), None),
        (
            "i8-null",
            crate::mysql::TypeLonglong,
            None,
            Some("FM000"),
            None,
        ),
        (
            "i4-null-template",
            crate::mysql::TypeLong,
            Some(12i64),
            None,
            None,
        ),
        (
            "i8-null-template",
            crate::mysql::TypeLonglong,
            Some(12i64),
            None,
            None,
        ),
        (
            "fm-mi",
            crate::mysql::TypeLonglong,
            Some(12i64),
            Some("FM999MI"),
            Some("12"),
        ),
        (
            "quoted-mi",
            crate::mysql::TypeLong,
            Some(12i64),
            Some("FM999\"MI\""),
            Some("12MI"),
        ),
        (
            "spaces",
            crate::mysql::TypeLonglong,
            Some(12i64),
            Some("  FM999  "),
            Some("  12  "),
        ),
        (
            "empty",
            crate::mysql::TypeLong,
            Some(1i64),
            Some(""),
            Some(""),
        ),
        (
            "zero",
            crate::mysql::TypeLonglong,
            Some(0i64),
            Some("FM000"),
            Some("000"),
        ),
    ] {
        let mut source = source
            .map(crate::NewInt64Const)
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
    for tp in [crate::mysql::TypeLong, crate::mysql::TypeLonglong] {
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
    println!("pg08_15_formal: 19 scenarios passed");
}
