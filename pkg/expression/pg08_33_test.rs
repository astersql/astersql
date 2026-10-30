// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};
#[test]
fn pg08_33_formal() {
    for (value, pattern, escape, locale, wanted) in [
        (Some("Ab"), Some("a%"), Some("\\"), "C", Some(1)),
        (Some("A_b"), Some("a!_b"), Some("!"), "C", Some(1)),
        (Some("A_b"), Some("a_b"), Some(""), "C", Some(1)),
        (Some("Ab"), Some("Ab\\"), Some("\\"), "C", Some(0)),
        (Some(""), Some("\\"), Some("\\"), "C", Some(0)),
        (Some("Ä"), Some("ä"), Some(""), "C", Some(0)),
        (Some("Ä"), Some("ä"), Some(""), "en_US.UTF-8", Some(1)),
        (Some("A_b"), Some("aé_b"), Some("é"), "en_US.UTF-8", Some(1)),
        (None, Some("a%"), Some("!"), "C", None),
        (Some("Ab"), None, Some("!"), "C", None),
        (Some("Ab"), Some("a%"), None, "C", None),
    ] {
        let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(
            std::sync::Arc::new(exprstatic::NewEvalContext(vec![
                exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
            ])),
        )]);
        let args = [value, pattern, escape]
            .into_iter()
            .map(|text| {
                let mut arg = text.map_or_else(crate::NewNull, crate::NewStrConst);
                let mut tp = crate::types::NewFieldType(crate::mysql::TypeVarString);
                tp.SetCollate(locale.to_owned());
                arg.RetType = Some(*tp);
                Box::new(arg) as Box<dyn Expression>
            })
            .collect();
        let f = crate::NewFunctionBase(
            &context,
            "ilike",
            *crate::types::NewFieldType(crate::mysql::TypeTiny),
            args,
        )
        .unwrap();
        let result = f
            .EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(
            if result.1 { None } else { Some(result.0) },
            wanted,
            "{value:?} {pattern:?} escape={escape:?} locale={locale}"
        );
    }
    for (pattern, escape, diagnostic) in [
        ("a%", "!!", "invalid escape string"),
        (
            "A\\",
            "\\",
            "LIKE pattern must not end with escape character",
        ),
    ] {
        let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(
            std::sync::Arc::new(exprstatic::NewEvalContext(vec![
                exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
            ])),
        )]);
        let f = crate::NewFunctionBase(
            &context,
            "ilike",
            *crate::types::NewFieldType(crate::mysql::TypeTiny),
            vec![
                Box::new(crate::NewStrConst("Ab")),
                Box::new(crate::NewStrConst(pattern)),
                Box::new(crate::NewStrConst(escape)),
            ],
        )
        .unwrap();
        assert!(
            f.EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap_err()
                .to_string()
                .contains(diagnostic)
        );
    }
    println!("pg08_33_formal: 13 scenarios");
}
