// Copyright 2026 AsterSQL.

use super::rule_prune_indexes::{
    AccessPath, DataSource, IndexColumn, IndexInfo, IndexMergeHint,
    prune_indexes_by_where_and_order,
};

fn index_path(id: u64, columns: &[usize], full_columns: Option<&[i64]>) -> AccessPath {
    AccessPath {
        id,
        index: Some(IndexInfo {
            id: id as i64,
            name: format!("idx_{id}"),
            columns: columns
                .iter()
                .copied()
                .map(|offset| IndexColumn { offset })
                .collect(),
            multi_value: false,
            condition_expression: None,
            affected_column_offsets: Vec::new(),
        }),
        table_path: false,
        forced: false,
        full_index_columns: full_columns
            .map(|columns| columns.iter().copied().map(Some).collect::<Vec<_>>()),
        single_scan: false,
    }
}

#[test]
fn static_pruning_does_not_invent_consecutive_prefixes() {
    let source = DataSource {
        table_columns: (1..=20).collect(),
        ..DataSource::default()
    };
    let offsets = [0, 1, 2, 3, 4, 0, 5, 6, 7, 8, 9];
    let paths = offsets
        .iter()
        .enumerate()
        .map(|(position, offset)| index_path(position as u64 + 1, &[*offset], None))
        .collect::<Vec<_>>();

    let kept = prune_indexes_by_where_and_order(&source, paths, &(1..=10).collect::<Vec<_>>(), 10);

    // Go cannot derive ordering diversity without FullIdxCols. The duplicate
    // static path therefore remains eligible instead of being rejected as an
    // already-seen consecutive prefix.
    assert_eq!(
        kept.iter().map(|path| path.id).collect::<Vec<_>>(),
        (1..=10).collect::<Vec<_>>()
    );
}

#[test]
fn empty_partial_index_condition_does_not_reject_the_path() {
    let source = DataSource {
        table_columns: vec![1],
        ..DataSource::default()
    };
    let mut useful = index_path(1, &[0], Some(&[1]));
    useful.index.as_mut().unwrap().condition_expression = Some(String::new());
    useful.index.as_mut().unwrap().affected_column_offsets = vec![99];
    let useless = index_path(2, &[], Some(&[2]));

    let kept = prune_indexes_by_where_and_order(&source, vec![useful, useless], &[1], 0);

    assert_eq!(kept.iter().map(|path| path.id).collect::<Vec<_>>(), vec![1]);
}

#[test]
fn forced_multi_value_path_does_not_disable_regular_pruning() {
    let source = DataSource::default();
    let mut multi_value = index_path(1, &[], None);
    multi_value.forced = true;
    multi_value.index.as_mut().unwrap().multi_value = true;
    let useful = index_path(2, &[0], Some(&[7]));
    let useless = index_path(3, &[0], Some(&[8]));

    let kept =
        prune_indexes_by_where_and_order(&source, vec![multi_value, useful, useless], &[7], 0);

    assert_eq!(
        kept.iter().map(|path| path.id).collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn index_merge_hint_names_use_unicode_case_insensitive_matching() {
    let source = DataSource {
        index_merge_hints: vec![IndexMergeHint {
            index_names: vec!["Å_Σ_IDX".to_owned()],
        }],
        ..DataSource::default()
    };
    let mut hinted = index_path(1, &[], Some(&[9]));
    hinted.index.as_mut().unwrap().name = "å_ς_idx".to_owned();
    let useful = index_path(2, &[], Some(&[7]));

    let kept = prune_indexes_by_where_and_order(&source, vec![hinted, useful], &[7], 0);

    assert_eq!(
        kept.iter().map(|path| path.id).collect::<Vec<_>>(),
        vec![1, 2]
    );
}
