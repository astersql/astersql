// Copyright 2026 AsterSQL.

use std::collections::HashMap;

use crate::{ColumnReplaceMap, ResolveColumnAndReplace};
use expression::{Column, Expression};

fn column(unique_id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = unique_id;
    column.ID = unique_id;
    column
}

#[test]
fn column_replace_map_preserves_binary_hash_keys() {
    let first = column(0x80);
    let second = column(0x81);
    let first_hash = Expression::HashCode(&first);
    let second_hash = Expression::HashCode(&second);
    assert_ne!(first_hash, second_hash);

    let replacements: ColumnReplaceMap =
        HashMap::from([(first_hash, column(10)), (second_hash, column(20))]);

    assert_eq!(ResolveColumnAndReplace(&first, &replacements).UniqueID, 10);
    assert_eq!(ResolveColumnAndReplace(&second, &replacements).UniqueID, 20);
}
