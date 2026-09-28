// Copyright 2026 AsterSQL.

use crate::*;

fn column(id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = id;
    column
}

#[test]
fn bit_and_grouping_id_marks_active_columns_like_go() {
    let a = column(1);
    let b = column(2);
    let expand = LogicalExpand {
        DistinctGroupByCol: vec![a.clone(), b],
        ..LogicalExpand::default()
    };

    assert_eq!(
        expand.GenerateGroupingIDModeBitAnd(&[a.UniqueID].into_iter().collect()),
        1
    );
}

#[test]
fn numeric_grouping_ids_deduplicate_sets_and_marks_track_active_levels() {
    let a = column(11);
    let b = column(12);
    let only_a = GroupingSet {
        ColumnIDs: [a.UniqueID].into_iter().collect(),
    };
    let only_b = GroupingSet {
        ColumnIDs: [b.UniqueID].into_iter().collect(),
    };
    let mut expand = LogicalExpand {
        DistinctGroupByCol: vec![a.clone(), b.clone()],
        RollupGroupingSets: GroupingSets(vec![only_a.clone(), only_a, only_b]),
        GroupingMode: GroupingMode::ModeNumericSet,
        ..LogicalExpand::default()
    };

    expand.GenerateGroupingMarks();

    assert_eq!(expand.DistinctSize, 2);
    assert_eq!(expand.RollupGroupingIDs, vec![0, 0, 1]);
    assert_eq!(
        expand.RollupID2GIDS.get(&a.UniqueID),
        Some(&[0].into_iter().collect())
    );
    assert_eq!(
        expand.RollupID2GIDS.get(&b.UniqueID),
        Some(&[1].into_iter().collect())
    );
}

#[test]
fn expand_reports_no_intrinsic_used_columns_like_go() {
    let expand = LogicalExpand {
        DistinctGroupByCol: vec![column(21)],
        DistinctGbyExprs: vec![Box::new(column(22))],
        ..LogicalExpand::default()
    };

    assert!(expand.GetUsedCols().is_empty());
}
