// Copyright 2026 AsterSQL.
use crate::Expression;
#[test]
fn pg08_21_formal() {
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let cases = [
        ("empty", Some("{}"), Some(7), Some("{7}"), ""),
        ("null-array", None, Some(7), Some("{7}"), ""),
        ("null-both", None, None, Some("{NULL}"), ""),
        (
            "null-element",
            Some("{1,NULL}"),
            None,
            Some("{1,NULL,NULL}"),
            "",
        ),
        (
            "lower-zero",
            Some("[0:1]={10,20}"),
            Some(7),
            Some("[0:2]={10,20,7}"),
            "",
        ),
        (
            "lower-negative",
            Some("[-2:-1]={10,20}"),
            Some(7),
            Some("[-2:0]={10,20,7}"),
            "",
        ),
        (
            "near-upper",
            Some("[2147483645:2147483645]={1}"),
            Some(7),
            Some("[2147483645:2147483646]={1,7}"),
            "",
        ),
        ("multidim", Some("{{1,2},{3,4}}"), Some(7), None, "22000"),
        (
            "upper-boundary",
            Some("[2147483646:2147483646]={1}"),
            Some(7),
            None,
            "54000",
        ),
    ];
    for (id, source, element, wanted, state) in cases {
        let mut array = source
            .map(crate::NewStrConst)
            .unwrap_or_else(crate::NewNull);
        let mut tp = *crate::types::NewFieldType(crate::mysql::TypeVarString);
        tp.SetPostgreSQLArrayElement(types_dependency::field::ast::PostgreSQLArrayElement::Int4);
        array.RetType = Some(tp.clone());
        let mut item = element
            .map(crate::NewInt64Const)
            .unwrap_or_else(crate::NewNull);
        item.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeLong));
        let expr = crate::NewFunctionBase(
            &context,
            "array_append",
            tp,
            vec![Box::new(array.clone()), Box::new(item)],
        )
        .unwrap();
        assert_eq!(
            expr.GetType(context.GetEvalCtx()).PostgreSQLArrayElement(),
            Some(types_dependency::field::ast::PostgreSQLArrayElement::Int4)
        );
        let result = expr.EvalString(context.GetEvalCtx(), crate::chunk::Row::default());
        if state.is_empty() {
            let (value, null) = result.unwrap();
            assert!(!null, "{id}");
            assert_eq!(Some(value.as_str()), wanted, "{id}");
            let parse = |text: &str| {
                types_dependency::pg_array::PgArray::from_text_with(
                    text.as_bytes(),
                    23,
                    -1,
                    b',',
                    |s, _| {
                        crate::pg_cast_int4(Some(s))
                            .map(|n| types_dependency::pg_array::PgArrayElement::Int4(n.unwrap()))
                    },
                )
                .unwrap()
            };
            let actual = parse(&value);
            let expected = parse(wanted.unwrap());
            assert_eq!(actual.shape(), expected.shape());
            assert_eq!(actual.lower_bounds(), expected.lower_bounds());
            assert_eq!(actual.elements(), expected.elements());
        } else {
            let error = result.unwrap_err();
            let wanted_message = if state == "22000" {
                "argument must be empty or one-dimensional array"
            } else {
                "array lower bound is too large: 2147483646"
            };
            assert_eq!(error.to_string(), wanted_message, "{id}");
            let shared = error.into_shared();
            let diagnostic = shared
                .downcast_ref::<crate::formal_registry::PgArrayAppendError>()
                .unwrap();
            assert_eq!(diagnostic.state, state, "{id}");
        }
        let after = array
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(
            (!after.1).then_some(after.0.as_str()),
            source,
            "{id}: input changed"
        );
    }
    // A plain text source and an array-valued element do not match int4[] + int4.
    // The int8 promotion signature belongs to batch 22 and is not constrained here.
    for array_element in [false, true] {
        let mut array = crate::NewStrConst("{1}");
        let mut source_type = *crate::types::NewFieldType(crate::mysql::TypeVarString);
        if array_element {
            source_type.SetPostgreSQLArrayElement(
                types_dependency::field::ast::PostgreSQLArrayElement::Int4,
            );
        }
        array.RetType = Some(source_type);
        let mut item = crate::NewStrConst(if array_element { "{7}" } else { "7" });
        let mut item_type = *crate::types::NewFieldType(if array_element {
            crate::mysql::TypeVarString
        } else {
            crate::mysql::TypeLong
        });
        if array_element {
            item_type.SetPostgreSQLArrayElement(
                types_dependency::field::ast::PostgreSQLArrayElement::Int4,
            );
        }
        item.RetType = Some(item_type);
        assert!(
            crate::NewFunctionBase(
                &context,
                "array_append",
                *crate::types::NewFieldType(crate::mysql::TypeVarString),
                vec![Box::new(array), Box::new(item)]
            )
            .is_err()
        );
    }
    println!("pg08_21_formal: 11 scenarios passed");
}
