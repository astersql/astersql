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

#[test]
fn identical_coverage_prefers_longer_prefix_and_narrower_key() {
    let source = DataSource {
        table_columns: vec![1, 2, 3, 4],
        ..Default::default()
    };
    let paths = vec![
        index_path(1, &[0, 1, 2, 3], Some(&[1, 2, 3, 4])),
        index_path(2, &[0, 1, 2], Some(&[1, 2, 3])),
        index_path(3, &[0, 3, 1, 2], Some(&[1, 4, 2, 3])),
    ];
    let kept = prune_indexes_by_where_and_order(&source, paths, &[1, 2, 3], 1);
    assert_eq!(kept.iter().map(|path| path.id).collect::<Vec<_>>(), vec![2]);
}

#[test]
fn clustered_prefix_only_indexes_are_redundant_but_covering_and_hinted_paths_survive() {
    let mut source = DataSource {
        table_columns: vec![1, 2, 3],
        ..Default::default()
    };
    source.discounted_column_ids.insert(1);
    let mut table = index_path(0, &[], None);
    table.table_path = true;
    table.index = None;
    let redundant = index_path(1, &[0, 1], Some(&[1, 2]));
    let mut covering = index_path(2, &[0, 2], Some(&[1, 3]));
    covering.single_scan = true;
    for threshold in [0, 1, 20] {
        let kept = prune_indexes_by_where_and_order(
            &source,
            vec![table.clone(), redundant.clone(), covering.clone()],
            &[1],
            threshold,
        );
        assert_eq!(
            kept.iter().map(|path| path.id).collect::<Vec<_>>(),
            vec![0, 2]
        );
    }
    source.index_merge_hints = vec![IndexMergeHint {
        index_names: vec!["idx_1".into()],
    }];
    let kept = prune_indexes_by_where_and_order(&source, vec![table, redundant], &[1], 0);
    assert_eq!(
        kept.iter().map(|path| path.id).collect::<Vec<_>>(),
        vec![0, 1]
    );
}

#[test]
fn appended_handle_keeps_different_access_orders() {
    let mut source = DataSource {
        table_columns: vec![1, 2, 3],
        ..Default::default()
    };
    let tenant_first = index_path(1, &[0, 1], Some(&[1, 2]));
    let value_first = index_path(2, &[1], Some(&[2]));
    source
        .effective_index_columns
        .insert(2, vec![Some(2), Some(1), Some(3)]);
    let wider = index_path(3, &[1, 2], Some(&[2, 3]));
    source
        .effective_index_columns
        .insert(3, vec![Some(2), Some(3), Some(1)]);
    let kept = prune_indexes_by_where_and_order(
        &source,
        vec![tenant_first, value_first, wider],
        &[1, 2],
        1,
    );
    assert_eq!(
        kept.iter().map(|path| path.id).collect::<Vec<_>>(),
        vec![2, 1]
    );
}

#[test]
fn partial_index_bad_constraint_offsets_have_zero_coverage() {
    let source = DataSource {
        table_columns: vec![1],
        ..Default::default()
    };
    for offset in [1, usize::MAX] {
        let mut bad = index_path(1, &[0], Some(&[1]));
        bad.index.as_mut().unwrap().condition_expression = Some("a > 0".into());
        bad.index.as_mut().unwrap().affected_column_offsets = vec![offset];
        let kept = prune_indexes_by_where_and_order(
            &source,
            vec![bad, index_path(2, &[0], Some(&[1]))],
            &[1],
            0,
        );
        assert_eq!(kept.iter().map(|path| path.id).collect::<Vec<_>>(), vec![2]);
    }
}
