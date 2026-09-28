// Copyright 2026 AsterSQL.

use std::collections::HashMap;

use crate::{BuildRuntimeTableStats, MergeRuntimePartitionHistograms, TableStats};

fn indexed_table() -> astersql_meta_model::TableInfo {
    let mut column = astersql_meta_model::ColumnInfo::default();
    column.ID = 1;
    column.Name.O = "a".to_owned();
    column.Name.L = "a".to_owned();
    column.FieldType.SetType(3);

    let mut index = astersql_meta_model::IndexInfo::default();
    index.ID = 2;
    index.Name.O = "idx_a".to_owned();
    index.Name.L = "idx_a".to_owned();
    index.Columns = vec![astersql_meta_model::IndexColumn {
        Name: column.Name.clone(),
        Offset: 0,
        Length: -1,
        ..Default::default()
    }];

    astersql_meta_model::TableInfo {
        ID: 10,
        Columns: vec![column],
        Indices: vec![index],
        ..Default::default()
    }
}

fn rows(values: &[&str]) -> Vec<HashMap<String, Option<String>>> {
    values
        .iter()
        .map(|value| HashMap::from([("a".to_owned(), Some((*value).to_owned()))]))
        .collect()
}

fn histogram_rows(stats: &TableStats, index_id: i64) -> i64 {
    stats.indexes[&index_id]
        .buckets
        .last()
        .map_or(0, |bucket| bucket.count)
        + stats.indexes[&index_id].null_count
}

fn column_histogram_rows(stats: &TableStats, column_id: i64) -> i64 {
    stats.columns[&column_id]
        .buckets
        .last()
        .map_or(0, |bucket| bucket.count)
        + stats.columns[&column_id].null_count
}

#[test]
fn merged_index_histogram_excludes_values_selected_for_global_top_n() {
    let table = indexed_table();
    let partition_rows = [
        rows(&["1", "1", "1", "2", "2"]),
        rows(&["1", "1", "2", "2", "2"]),
    ];
    let partitions = partition_rows
        .iter()
        .enumerate()
        .map(|(offset, rows)| BuildRuntimeTableStats(offset as i64, &table, rows, 1, 1).unwrap())
        .collect::<Vec<_>>();
    let all_rows = partition_rows.concat();
    let mut global = BuildRuntimeTableStats(10, &table, &all_rows, 1, 1).unwrap();

    MergeRuntimePartitionHistograms(
        &astersql_statistics::RuntimeStatsBuilder::default(),
        &table,
        &mut global,
        &partitions,
        256,
    )
    .unwrap();

    let top_n_rows = global.indexes[&2]
        .top_n
        .iter()
        .map(|(_, count)| *count as i64)
        .sum::<i64>();
    assert_eq!(
        histogram_rows(&global, 2) + top_n_rows,
        all_rows.len() as i64
    );
}

#[test]
fn merged_column_histogram_keeps_stats_associated_with_their_partition() {
    let table = indexed_table();
    let partition_rows = [rows(&["1"; 10]), rows(&["1", "1", "2", "2", "2"])];
    let mut partitions = partition_rows
        .iter()
        .enumerate()
        .map(|(offset, rows)| BuildRuntimeTableStats(offset as i64, &table, rows, 1, 1).unwrap())
        .collect::<Vec<_>>();
    partitions[0].columns.remove(&1);
    let all_rows = partition_rows.concat();
    let mut global = BuildRuntimeTableStats(10, &table, &all_rows, 1, 1).unwrap();

    MergeRuntimePartitionHistograms(
        &astersql_statistics::RuntimeStatsBuilder::default(),
        &table,
        &mut global,
        &partitions,
        256,
    )
    .unwrap();

    let top_n_rows = global.columns[&1]
        .top_n
        .iter()
        .map(|(_, count)| *count as i64)
        .sum::<i64>();
    assert_eq!(
        column_histogram_rows(&global, 1) + top_n_rows,
        all_rows.len() as i64
    );
}
