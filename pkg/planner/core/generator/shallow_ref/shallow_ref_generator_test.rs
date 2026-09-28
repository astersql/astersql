// Copyright 2026 AsterSQL.

use super::{GenShallowRef4LogicalOps, refine_field_type_name};

#[test]
fn generator_matches_go_output() {
    let generated = GenShallowRef4LogicalOps().expect("generator must succeed");
    let expected = include_bytes!("../../operator/logicalop/shallow_ref_generated.go");

    assert_eq!(generated.as_slice(), expected);
}

#[test]
fn generator_preserves_field_level_and_recursive_copy_semantics() {
    let generated = GenShallowRef4LogicalOps().expect("generator must succeed");
    let generated = String::from_utf8(generated).expect("Go source must be UTF-8");

    assert!(generated.contains(
        "func (op *LogicalJoin) EqualConditionsShallowRef() []*expression.ScalarFunction"
    ));
    assert!(generated.contains("PossiblePropertiesCP := op.PossibleProperties"));
    assert!(generated.contains("oneCP := make([]*expression.Column, 0, len(one))"));
    assert!(generated.contains("PossiblePropertiesCP.Orders = OrdersCP"));
}

#[test]
fn refine_field_type_name_matches_go_prefix_rule() {
    assert_eq!(
        refine_field_type_name("logicalop.LogicalJoin"),
        "LogicalJoin"
    );
    assert_eq!(
        refine_field_type_name("base.PossiblePropertiesInfo"),
        "base.PossiblePropertiesInfo"
    );
}
