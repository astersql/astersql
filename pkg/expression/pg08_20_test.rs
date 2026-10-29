// Copyright 2026 AsterSQL.
use crate::Expression;
#[test]
fn pg08_20_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let cases = [
        (Some("{}"), Some(0)),
        (None, None),
        (Some("{1,NULL}"), Some(2)),
        (Some("{{1,2,3},{4,5,6}}"), Some(6)),
        (Some("[0:1]={10,20}"), Some(2)),
        (Some("[-2:-1]={10,20}"), Some(2)),
    ];
    for (source, wanted) in cases {
        let mut array = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        let mut tp = *crate::types::NewFieldType(crate::mysql::TypeVarString);
        tp.SetPostgreSQLArrayElement(types_dependency::field::ast::PostgreSQLArrayElement::Int4);
        array.RetType = Some(tp);
        let expr = crate::NewFunctionBase(
            &context,
            "cardinality",
            *crate::types::NewFieldType(crate::mysql::TypeLong),
            vec![Box::new(array.clone())],
        )
        .unwrap();
        assert_eq!(
            expr.GetType(context.GetEvalCtx()).GetType(),
            crate::mysql::TypeLong
        );
        let (value, null) = expr
            .EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!null).then_some(value), wanted, "{source:?}");
        let after = array
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!((!after.1).then_some(after.0.as_str()), source);
    }
    // JSON/text is not a typed PostgreSQL array, even when its value looks like one.
    assert!(
        crate::NewFunctionBase(
            &context,
            "cardinality",
            *crate::types::NewFieldType(crate::mysql::TypeLong),
            vec![Box::new(crate::NewStrConst("{1,2}"))]
        )
        .is_err()
    );
    println!("pg08_20_formal: 7 scenarios passed");
}
