// Copyright 2026 AsterSQL.

use crate::*;

fn column(id: i64, unique_id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        id,
        unique_id,
        0,
    )
}

#[test]
fn build_key_info_clears_stale_keys_without_a_single_child() {
    let stale = column(1, 101);
    let mut producer = LogicalSchemaProducer::default();
    producer.SetSchema(expression::NewSchema(vec![stale.clone()]));
    producer.Schema_mut().PKOrUK = vec![vec![stale]];

    producer.BuildKeyInfo();
    assert!(producer.Schema().PKOrUK.is_empty());

    let mut left = LogicalSchemaProducer::default();
    let left_key = column(2, 102);
    left.SetSchema(expression::NewSchema(vec![left_key.clone()]));
    left.Schema_mut().PKOrUK = vec![vec![left_key]];
    let right = LogicalSchemaProducer::default();
    producer.SetChildren(vec![Box::new(left), Box::new(right)]);
    producer.Schema_mut().PKOrUK = vec![vec![column(3, 103)]];

    producer.BuildKeyInfo();
    assert!(producer.Schema().PKOrUK.is_empty());
}

#[test]
fn build_key_info_maps_child_keys_to_output_columns() {
    let child_key = column(7, 107);
    let mut child = LogicalSchemaProducer::default();
    child.SetSchema(expression::NewSchema(vec![child_key.clone()]));
    child.Schema_mut().PKOrUK = vec![vec![child_key]];

    let output_column = column(70, 107);
    let mut producer = LogicalSchemaProducer::default();
    producer.SetSchema(expression::NewSchema(vec![output_column]));
    producer.SetChildren(vec![Box::new(child)]);

    producer.BuildKeyInfo();

    assert_eq!(producer.Schema().PKOrUK.len(), 1);
    assert_eq!(producer.Schema().PKOrUK[0][0].ID, 70);
}
