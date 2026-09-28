// Copyright 2026 AsterSQL.

use crate::{
    ActionType, CIStr, IndexColumn, IndexInfo, IsIndexPrefixCoveredForForeignKey, JobState,
    ReorgType, TableInfo, UnspecifiedLength,
};

#[test]
fn model_enum_discriminants_match_go() {
    assert_eq!(ReorgType::None as i8, 0);
    assert_eq!(ReorgType::Txn as i8, 1);
    assert_eq!(ReorgType::Ingest as i8, 2);
    assert_eq!(ReorgType::TxnMerge as i8, 3);

    assert_eq!(ActionType::Other as u8, 0);
    assert_eq!(ActionType::AddIndex as u8, 7);
    assert_eq!(ActionType::DropIndex as u8, 8);
    assert_eq!(ActionType::ModifyColumn as u8, 12);
    assert_eq!(ActionType::AddPrimaryKey as u8, 32);

    assert_eq!(JobState::None as i32, 0);
    assert_eq!(JobState::RollbackDone as i32, 3);
    assert_eq!(JobState::Done as i32, 4);
    assert_eq!(JobState::Synced as i32, 6);
}

#[test]
fn qualified_partial_index_column_matches_go_ast() {
    let table = TableInfo {
        Columns: vec![crate::ColumnInfo {
            Name: CIStr::new("child_id"),
            Flen: 32,
            ..Default::default()
        }],
        ..Default::default()
    };
    let index = IndexInfo {
        Columns: vec![IndexColumn {
            Name: CIStr::new("child_id"),
            Offset: 0,
            Length: UnspecifiedLength,
        }],
        ConditionExprString: "`child`.`child_id` IS NOT NULL".to_string(),
        ..Default::default()
    };

    assert!(IsIndexPrefixCoveredForForeignKey(
        &table,
        &index,
        &[CIStr::new("child_id")],
    ));
}
