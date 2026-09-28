// Copyright 2026 AsterSQL.

use crate::generated_column::{
    ExpressionNode, GeneratedColumnError, GenerationType, check_illegal_function_for_generated,
};

fn function(name: &str, arguments: Vec<ExpressionNode>) -> ExpressionNode {
    ExpressionNode::Function {
        name: name.into(),
        supported: true,
        guaranteed_available: true,
        arguments,
    }
}

#[test]
fn grouping_is_reported_as_an_aggregate_function_like_go() {
    let expression = function("GROUPING", vec![ExpressionNode::Column("a".into())]);

    assert_eq!(
        check_illegal_function_for_generated("g", GenerationType::Column, &expression, false,),
        Err(GeneratedColumnError::AggregateFunction)
    );
}

#[test]
fn expression_index_rejects_array_cast_below_a_function_like_go() {
    let expression = function(
        "identity",
        vec![ExpressionNode::CastArray(Box::new(ExpressionNode::Column(
            "a".into(),
        )))],
    );

    assert_eq!(
        check_illegal_function_for_generated("idx", GenerationType::Index, &expression, true),
        Err(GeneratedColumnError::CastArray)
    );
}

#[test]
fn expression_index_allows_a_root_array_cast_like_go() {
    let expression = ExpressionNode::CastArray(Box::new(ExpressionNode::Column("a".into())));

    assert_eq!(
        check_illegal_function_for_generated("idx", GenerationType::Index, &expression, true),
        Ok(())
    );
}
