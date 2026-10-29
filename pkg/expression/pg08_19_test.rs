// Copyright 2026 AsterSQL.
use crate::Expression;
#[test]
fn pg08_19_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let cases = [
        (Some("{}"), Some(1), None),
        (None, Some(1), None),
        (Some("{1,NULL}"), None, None),
        (Some("{1,NULL}"), Some(1), Some(2)),
        (Some("{{1,2,3},{4,5,6}}"), Some(1), Some(2)),
        (Some("{{1,2,3},{4,5,6}}"), Some(2), Some(3)),
        (Some("{{1,2,3},{4,5,6}}"), Some(3), None),
        (Some("{{1,2,3},{4,5,6}}"), Some(0), None),
        (Some("{{1,2,3},{4,5,6}}"), Some(-1), None),
        (Some("[0:1]={10,20}"), Some(1), Some(2)),
        (Some("[-2:-1]={10,20}"), Some(1), Some(2)),
    ];
    for (source, dimension, wanted) in cases {
        let mut array = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        let mut tp = *crate::types::NewFieldType(crate::mysql::TypeVarString);
        tp.SetPostgreSQLArrayElement(types_dependency::field::ast::PostgreSQLArrayElement::Int4);
        array.RetType = Some(tp);
        let mut dim = dimension
            .map(|n| crate::NewInt64Const(n))
            .unwrap_or_else(crate::NewNull);
        dim.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeLong));
        let expr = crate::NewFunctionBase(
            &context,
            "array_length",
            *crate::types::NewFieldType(crate::mysql::TypeLong),
            vec![Box::new(array.clone()), Box::new(dim)],
        )
        .unwrap();
        assert_eq!(
            expr.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypeLong
        );
        let (value, null) = expr
            .EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!null).then_some(value), wanted, "{source:?}/{dimension:?}");
        let after = array
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!after.1).then_some(after.0.as_str()), source);
    }
    // JSON/text is not a typed PostgreSQL array, even when its value looks like one.
    let mut dimension = crate::NewInt64Const(1);
    dimension.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeLong));
    assert!(
        crate::NewFunctionBase(
            &context,
            "array_length",
            *crate::types::NewFieldType(crate::mysql::TypeLong),
            vec![Box::new(crate::NewStrConst("{1,2}")), Box::new(dimension)]
        )
        .is_err()
    );
    println!("pg08_19_formal: 12 scenarios passed");
}
