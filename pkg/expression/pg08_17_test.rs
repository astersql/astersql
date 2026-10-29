// Copyright 2026 AsterSQL.
use crate::Expression;
#[test]
fn pg08_17_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (template, wanted) in [
        ("TMMonth", Some("January")),
        ("TMDay", Some("Tuesday")),
        ("FMMonth FMDay", Some("January Tuesday")),
        ("\"TMMonth\" TMMon", Some("TMMonth Jan")),
        ("TMmonth TMday", Some("january tuesday")),
        ("TMMONTH TMDAY", Some("JANUARY TUESDAY")),
        ("FMTMMonth|TMFMMonth", Some("T01onth|F01onth")),
        ("", None),
    ] {
        let mut source = crate::NewStrConst("2024-01-02 00:00:00");
        source.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeDatetime));
        let expr = crate::NewFunctionBase(
            &context,
            "to_char",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![Box::new(source), Box::new(crate::NewStrConst(template))],
        )
        .unwrap();
        let expr = crate::FoldConstant(&context, expr);
        assert!(expr.as_scalar_function().is_some());
        let (actual, null) = expr
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!null).then_some(actual.as_str()), wanted, "{template}");
    }
    for (locale, wanted) in [("C", "January Tuesday"), ("de_DE.UTF-8", "Januar Dienstag")] {
        let mut source = crate::NewStrConst("2024-01-02 00:00:00");
        source.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeDatetime));
        let expression = crate::NewFunctionBase(
            &context,
            "to_char",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![
                Box::new(source),
                Box::new(crate::NewStrConst("TMMonth TMDay")),
            ],
        )
        .unwrap();
        let eval = exprstatic::NewEvalContext(vec![
            exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
            exprstatic::WithPgLcTime(locale.into()),
        ]);
        let snapshot = exprstatic::MakeEvalContextStatic(&eval);
        assert_eq!(
            expression
                .EvalString(&snapshot, crate::chunk::Row::default())
                .unwrap(),
            (wanted.into(), false)
        );
    }
    for (template, wanted) in [
        ("TMMonth", "MäRz"),
        ("TMmonth", "märz"),
        ("TMMONTH", "MäRZ"),
    ] {
        let mut source = crate::NewStrConst("2024-03-03 00:00:00");
        source.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeDatetime));
        let expr = crate::NewFunctionBase(
            &context,
            "to_char",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![Box::new(source), Box::new(crate::NewStrConst(template))],
        )
        .unwrap();
        let eval = exprstatic::NewEvalContext(vec![
            exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
            exprstatic::WithPgLcTime("de_DE.UTF-8".into()),
        ]);
        assert_eq!(
            expr.EvalString(&eval, crate::chunk::Row::default())
                .unwrap(),
            (wanted.into(), false)
        );
    }
    println!("pg08_17_formal: 13 scenarios passed");
}
