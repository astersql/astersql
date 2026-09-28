// Copyright 2026 AsterSQL.

use crate::*;

fn column(id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = id;
    column
}

fn sort_item(id: i64) -> SortItem {
    SortItem {
        Col: column(id),
        Desc: false,
    }
}

#[test]
fn partition_equality_matches_go_set_semantics() {
    let left = LogicalWindow {
        PartitionBy: vec![sort_item(1), sort_item(2)],
        ..LogicalWindow::default()
    };
    let right = LogicalWindow {
        PartitionBy: vec![sort_item(2), sort_item(1)],
        ..LogicalWindow::default()
    };

    assert!(left.equalPartitionBy(&right));
}
