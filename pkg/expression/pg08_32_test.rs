// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};

#[test]
fn pg08_32_formal() {
    let pg_version = format!(
        "PostgreSQL 18.0 (AsterSQL) compatibility\n{}",
        crate::tidb_version()
    );
    for dialect in [
        crate::ast::SqlDialect::PostgreSQL,
        crate::ast::SqlDialect::MySQL,
        crate::ast::SqlDialect::PostgreSQL,
    ] {
        let context =
            exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
                exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(dialect)]),
            ))]);
        let expression = crate::NewFunctionBase(
            &context,
            "version",
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
            vec![],
        )
        .expect("version formal factory");
        assert_eq!(
            expression.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypeVarString
        );
        let expected = if dialect == crate::ast::SqlDialect::PostgreSQL {
            pg_version.clone()
        } else {
            crate::mysql::r#const::ServerVersion()
        };
        for _ in 0..2 {
            assert_eq!(
                expression
                    .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
                    .unwrap(),
                (expected.clone(), false)
            );
            assert_eq!(
                expression
                    .Eval(context.GetEvalCtx(), crate::chunk::Row::default())
                    .unwrap()
                    .ToString()
                    .unwrap(),
                expected
            );
        }
        assert!(!expression.SafeToShareAcrossSession());
        assert!(crate::function_traits_kernel::is_uncacheable("version"));
        assert!(
            crate::NewFunctionBase(
                &context,
                "version",
                *crate::types::NewFieldType(crate::mysql::TypeVarString),
                vec![Box::new(crate::NewOne())]
            )
            .is_err()
        );
    }
    let candidate = crate::formal_registry::resolve_pg_signature("version", &[]).unwrap();
    assert_eq!(candidate.result, crate::mysql::TypeVarString);
    assert_eq!(
        crate::formal_registry::resolve_pg_signature("version", &[crate::mysql::TypeLong])
            .unwrap_err(),
        "42883"
    );
    println!("pg08_32_formal: dialect factories, stable builds, arity and cache isolation passed");
}
