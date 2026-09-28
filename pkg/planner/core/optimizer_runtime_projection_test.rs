// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn ordered_union_removes_identity_output_projection() {
    let plan = crate::main_test::optimize_query_for_test(
        "select * from ((SELECT 1 a,6 b) UNION (SELECT 2,5) UNION (SELECT 2,4) ORDER BY 1) t order by 1,2",
    )
    .expect("optimize ordered derived UNION");
    assert_eq!(plan.tp(&[]), "Sort", "{}", plan.explain_info());
    assert_eq!(plan.children()[0].tp(&[]), "Sort");
    let output = plan
        .schema()
        .Columns
        .iter()
        .map(|column| column.UniqueID)
        .collect::<Vec<_>>();
    let child_output = plan.children()[0]
        .schema()
        .Columns
        .iter()
        .map(|column| column.UniqueID)
        .collect::<Vec<_>>();
    assert_eq!(output, child_output);
    assert_eq!(output.len(), 2);
}

fn test_column(unique_id: i64) -> expression::Column {
    expression::Column::new(
        expression::types::FieldType::default(),
        unique_id,
        unique_id,
        0,
    )
}

#[test]
fn index_outer_projection_pruning_preserves_outer_hash_keys() {
    let nation_name = test_column(1);
    let supplier_key = test_column(2);
    let customer_key = test_column(3);
    let mut required = vec![nation_name.Clone(), customer_key.Clone()];
    let outer_hash_keys = vec![supplier_key.Clone()];

    extend_required_columns(&mut required, &outer_hash_keys);

    assert!(
        required
            .iter()
            .any(|column| column.EqualColumn(&supplier_key))
    );
}

#[test]
fn index_outer_hash_key_comes_from_cross_child_equal_condition() {
    let supplier_key = test_column(10);
    let lineitem_key = test_column(11);
    let outer_schema = expression::NewSchema(vec![supplier_key.Clone()]);
    let inner_schema = expression::NewSchema(vec![lineitem_key.Clone()]);

    let outer_hash_key =
        outer_hash_key_for_equality(&supplier_key, &lineitem_key, &outer_schema, &inner_schema)
            .expect("one equality side belongs to each IndexJoin child");

    assert!(outer_hash_key.EqualColumn(&supplier_key));
}
