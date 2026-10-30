// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};
#[test]
fn pg08_31_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let expr = crate::NewFunctionBase(
        &context,
        "pg_backend_pid",
        *crate::types::NewFieldType(crate::mysql::TypeLong),
        vec![],
    )
    .expect("pg_backend_pid formal factory");
    assert_eq!(
        expr.GetType(context.GetEvalCtx()).GetType(),
        crate::mysql::TypeLong
    );
    assert!(
        expr.EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
            .is_err(),
        "missing backend identity must not become zero"
    );
    assert!(
        crate::NewFunctionBase(
            &context,
            "pg_backend_pid",
            *crate::types::NewFieldType(crate::mysql::TypeLong),
            vec![Box::new(crate::NewOne())]
        )
        .is_err()
    );
    let mysql = exprstatic::NewExprContext(vec![]);
    assert!(
        crate::NewFunctionBase(
            &mysql,
            "pg_backend_pid",
            *crate::types::NewFieldType(crate::mysql::TypeLong),
            vec![]
        )
        .is_err()
    );
    assert!(crate::unFoldableFunctions.contains_key("pg_backend_pid"));
    assert!(crate::function_traits_kernel::is_uncacheable(
        "pg_backend_pid"
    ));
    assert!(crate::function_traits_kernel::is_unfoldable(
        "pg_backend_pid"
    ));
    assert!(crate::function_traits_kernel::is_illegal_generated_column_function("pg_backend_pid"));
    assert!(!expr.SafeToShareAcrossSession());
    for pid in [1, 37, i32::MAX] {
        let eval = exprstatic::NewEvalContext(vec![
            exprstatic::WithSQLDialect(crate::ast::SqlDialect::PostgreSQL),
            exprstatic::WithPgBackendPid(Some(pid)),
        ]);
        for _ in 0..2 {
            assert_eq!(
                expr.EvalInt(&eval, crate::chunk::Row::default()).unwrap(),
                (i64::from(pid), false)
            );
            assert_eq!(
                expr.Eval(&eval, crate::chunk::Row::default())
                    .unwrap()
                    .ToString()
                    .unwrap(),
                pid.to_string()
            );
        }
        let snapshot = exprstatic::MakeEvalContextStatic(&eval);
        assert_eq!(
            expr.EvalInt(&snapshot, crate::chunk::Row::default())
                .unwrap(),
            (i64::from(pid), false)
        );
    }
    for invalid in [0, -1] {
        let eval = exprstatic::NewEvalContext(vec![exprstatic::WithPgBackendPid(Some(invalid))]);
        assert!(expr.EvalInt(&eval, crate::chunk::Row::default()).is_err());
    }
    assert_eq!(crate::pg_backend_pid(0), None);
    for id in [
        1,
        i32::MAX as u64,
        i32::MAX as u64 + 1,
        u32::MAX as u64,
        u64::MAX,
    ] {
        assert!((1..=i32::MAX).contains(&crate::pg_backend_pid(id).unwrap()));
    }
    println!(
        "pg08_31_formal: factory, invalid signatures, MySQL isolation, execution snapshots and identity boundaries passed"
    );
}
