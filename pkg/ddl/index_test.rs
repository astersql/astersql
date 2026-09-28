// Copyright 2026 AsterSQL.

use super::index::{
    AnalyzeStatus, ColumnInfo, ColumnType, ColumnarIndexType, IndexColumn, IndexError, IndexInfo,
    IndexKind, JobErrorKind, TableInfo, TaskKeyBuilder, adjust_concurrency,
    analyze_status_decision, build_index_columns, calc_bytes_length_for_decimal,
    check_and_build_index_condition_string, check_primary_key_on_generated_column,
    get_index_column_length, index_column_slice_equal, is_retryable_job_error, remove_index_info,
    rename_index, task_key,
};

fn column(column_type: ColumnType) -> ColumnInfo {
    ColumnInfo {
        id: 1,
        name: "c".to_owned(),
        column_type,
        charset_max_bytes: 1,
        generated: false,
        stored: false,
        hidden: false,
        nullable: false,
        primary_key: false,
        index_flags: 0,
        generated_dependencies: Default::default(),
    }
}

#[test]
fn decimal_storage_length_matches_go_packed_decimal_formula() {
    let cases = [
        (0, 0),
        (1, 1),
        (2, 1),
        (8, 4),
        (9, 4),
        (10, 5),
        (18, 8),
        (19, 9),
    ];
    for (precision, expected) in cases {
        assert_eq!(calc_bytes_length_for_decimal(precision), expected);
    }
}

#[test]
fn columnar_index_uses_go_minimum_nonzero_length() {
    let vector = column(ColumnType::Vector);
    assert_eq!(
        get_index_column_length(&vector, None, ColumnarIndexType::Vector),
        Ok(1)
    );
}

#[test]
fn temporal_storage_lengths_match_go_mysql_defaults() {
    let cases = [
        (ColumnType::Date, 3),
        (ColumnType::DateTime, 8),
        (ColumnType::Timestamp, 4),
        (ColumnType::Duration, 3),
    ];
    for (column_type, expected) in cases {
        assert_eq!(
            get_index_column_length(&column(column_type), None, ColumnarIndexType::None),
            Ok(expected)
        );
    }
}

#[test]
fn stored_generated_column_is_allowed_in_primary_key() {
    let mut stored = column(ColumnType::Int);
    stored.generated = true;
    stored.stored = true;
    assert_eq!(
        check_primary_key_on_generated_column(&[stored], &[("c".to_owned(), None)]),
        Ok(())
    );
}

#[test]
fn analyze_decision_tuple_matches_go_proceed_contract() {
    assert_eq!(
        analyze_status_decision(AnalyzeStatus::Finished, false),
        (true, false, false, false)
    );
    assert_eq!(
        analyze_status_decision(AnalyzeStatus::Failed, false),
        (true, false, true, false)
    );
    assert_eq!(
        analyze_status_decision(AnalyzeStatus::NotFound, false),
        (false, false, false, true)
    );
}

#[test]
fn job_retry_uses_go_next_error_threshold_and_unknown_policy() {
    assert!(is_retryable_job_error(JobErrorKind::Unknown, 3));
    assert!(!is_retryable_job_error(JobErrorKind::Unknown, 4));
    assert!(!is_retryable_job_error(JobErrorKind::WriteConflict, 4));
}

#[test]
fn repair_index_column_equality_ignores_prefix_length_like_go() {
    let left = [IndexColumn {
        name: "C".to_owned(),
        offset: 0,
        length: Some(3),
    }];
    let right = [IndexColumn {
        name: "c".to_owned(),
        offset: 8,
        length: Some(7),
    }];
    assert!(index_column_slice_equal(&left, &right));
}

#[test]
fn task_keys_match_go_label_order_and_spelling() {
    assert_eq!(task_key(42, false), "ddl/backfill/42");
    assert_eq!(task_key(42, true), "ddl/backfill/42/merge");
    assert_eq!(
        TaskKeyBuilder::new()
            .set_merge_temporary_index(true)
            .set_multi_schema(Some(7))
            .build(42),
        "ddl/backfill/42/7/merge"
    );
}

#[test]
fn concurrency_is_the_go_minimum_without_inventing_workers() {
    assert_eq!(adjust_concurrency(0, 8), 0);
    assert_eq!(adjust_concurrency(8, 0), 0);
    assert_eq!(adjust_concurrency(8, 3), 3);
}

#[test]
fn rename_index_allows_case_only_changes_like_go() {
    let mut table = TableInfo {
        indices: vec![IndexInfo {
            id: 1,
            name: "inDex".to_owned(),
            ..Default::default()
        }],
        ..Default::default()
    };
    rename_index(&mut table, "inDex", "IndEX").unwrap();
    assert_eq!(table.indices[0].name, "IndEX");
}

#[test]
fn non_prefixable_and_inverted_columns_match_go_validation() {
    let int = column(ColumnType::Int);
    assert_eq!(
        build_index_columns(
            &[int.clone()],
            &[("c".to_owned(), Some(1))],
            ColumnarIndexType::None
        ),
        Err(IndexError::UnsupportedIndexType)
    );
    let mut table = TableInfo {
        columns: vec![column(ColumnType::VarChar(10))],
        ..Default::default()
    };
    let result = super::index::build_index_info_for_deploy_mode(
        &mut table,
        "idx",
        &[("c".to_owned(), None)],
        super::index::IndexOptions {
            kind: IndexKind::Inverted,
            ..Default::default()
        },
        false,
    );
    assert_eq!(result, Err(IndexError::InvalidInvertedIndex));
}

#[test]
fn partial_index_rejects_stored_generated_columns_like_go() {
    let mut stored = column(ColumnType::Int);
    stored.generated = true;
    stored.stored = true;
    let table = TableInfo {
        columns: vec![stored],
        ..Default::default()
    };
    assert_eq!(
        check_and_build_index_condition_string(&table, &["c".to_owned()], "c > 0"),
        Err(IndexError::InvalidCondition)
    );
}

#[test]
fn removing_hidden_columns_repairs_remaining_index_offsets() {
    let mut hidden = column(ColumnType::Int);
    hidden.name = "hidden".to_owned();
    hidden.hidden = true;
    let mut visible = column(ColumnType::Int);
    visible.name = "visible".to_owned();
    let removed = IndexInfo {
        id: 1,
        name: "removed".to_owned(),
        columns: vec![IndexColumn {
            name: "hidden".to_owned(),
            offset: 0,
            length: None,
        }],
        ..Default::default()
    };
    let remaining = IndexInfo {
        id: 2,
        name: "remaining".to_owned(),
        columns: vec![IndexColumn {
            name: "visible".to_owned(),
            offset: 1,
            length: None,
        }],
        ..Default::default()
    };
    let mut table = TableInfo {
        columns: vec![hidden, visible],
        indices: vec![removed, remaining],
        ..Default::default()
    };
    remove_index_info(&mut table, "removed").unwrap();
    assert_eq!(table.columns.len(), 1);
    assert_eq!(table.indices[0].columns[0].offset, 0);
}
