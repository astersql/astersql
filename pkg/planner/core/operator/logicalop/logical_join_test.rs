// Copyright 2026 AsterSQL.

use crate::*;

fn column(id: i64, field_type: expression::types::FieldType) -> Column {
    let mut column = Column::default();
    column.UniqueID = id;
    column.RetType = Some(field_type);
    column
}

#[test]
fn redundant_column_resolution_matches_go_guards() {
    let redundant = column(
        2,
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
    );
    let visible = column(
        1,
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
    );
    let mut join = LogicalJoin::default();
    join.SetSchema(expression::NewSchema(vec![visible.clone()]));
    join.SetOutputNames(NameSlice(vec![None]));
    join.RegisterRedundantColumnMapping(redundant.UniqueID, 0);

    join.JoinType = JoinType::LeftOuterJoin;
    assert!(join.ResolveRedundantColumn(&redundant).is_none());

    join.JoinType = JoinType::InnerJoin;
    assert_eq!(
        join.ResolveRedundantColumn(&redundant)
            .map(|column| column.UniqueID),
        Some(visible.UniqueID)
    );

    join.SetOutputNames(NameSlice(Vec::new()));
    assert!(join.ResolveRedundantColumn(&redundant).is_none());

    join.SetOutputNames(NameSlice(vec![None]));
    let mut incompatible = redundant.clone();
    incompatible
        .RetType
        .as_mut()
        .expect("field type")
        .SetFlag(expression::mysql::UnsignedFlag);
    assert!(join.ResolveRedundantColumn(&incompatible).is_none());
}
