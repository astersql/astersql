// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};
#[test]
fn pg08_37_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (left, right, wanted) in [
        (Some("a"), Some("b"), Some("ab")),
        (None, Some("b"), None),
        (Some("a"), None, None),
        (None, None, None),
        (Some(""), Some("b"), Some("b")),
        (Some("a"), Some(""), Some("a")),
        (Some(""), Some(""), Some("")),
        (Some("你好"), Some("🌍"), Some("你好🌍")),
    ] {
        let args = [left, right]
            .into_iter()
            .map(|v| {
                let mut a = v.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
                let mut tp = *crate::types::NewFieldType(crate::mysql::TypeVarString);
                tp.SetPostgreSQLTextOid(25);
                a.RetType = Some(tp);
                Box::new(a) as Box<dyn Expression>
            })
            .collect();
        let f = crate::NewFunctionBase(
            &context,
            "||",
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            args,
        )
        .unwrap();
        let tp = f.GetType(context.GetEvalCtx());
        assert_eq!(tp.PostgreSQLTextOid(), Some(25));
        assert_eq!(tp.GetCollate(), "utf8mb4_bin");
        let (actual, null) = f
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!null).then_some(actual.as_str()), wanted);
        assert_eq!(f.as_scalar_function().unwrap().Function.PbCode(), 0);
    }
    assert!(
        crate::NewFunctionBase(
            &context,
            "||",
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            vec![
                Box::new(crate::NewInt64Const(1)),
                Box::new(crate::NewInt64Const(2))
            ]
        )
        .is_err()
    );
    println!("pg08_37_formal: 9 semantic cases");
}
