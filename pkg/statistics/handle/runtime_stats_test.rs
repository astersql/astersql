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
        // The missing first partition contributes no histogram or TopN
        // snapshot; preserve only the second partition's available mass.
        partition_rows[1].len() as i64
    );
}

#[test]
fn global_merge_selects_topn_from_partition_statistics_instead_of_prior_global_stats() {
    let table = indexed_table();
    let partition = BuildRuntimeTableStats(11, &table, &rows(&["2"; 5]), 1, 1).unwrap();
    // A prior global profile may name a different popular value. The Go
    // combined merge derives its output exclusively from partition inputs.
    let mut global = BuildRuntimeTableStats(10, &table, &rows(&["1"; 5]), 1, 1).unwrap();
    MergeRuntimePartitionHistograms(
        &astersql_statistics::RuntimeStatsBuilder::default(),
        &table,
        &mut global,
        std::slice::from_ref(&partition),
        256,
    )
    .unwrap();
    assert_eq!(global.columns[&1].top_n, partition.columns[&1].top_n);
    assert_eq!(global.indexes[&2].top_n, partition.indexes[&2].top_n);
}

#[test]
fn global_combined_merge_rebuilds_typed_persisted_bounds() {
    for (tp, values) in [
        (16_u8, ["1", "2", "256"]),
        (247, ["a", "z", "a"]),
        (248, ["a", "z", "a"]),
        (
            7,
            [
                "2001-01-01 00:00:00",
                "2001-01-02 00:00:00",
                "2001-01-01 00:00:00",
            ],
        ),
    ] {
        let mut table = indexed_table();
        table.Columns[0].FieldType.SetType(tp);
        if tp == 16 {
            table.Columns[0].FieldType.SetFlen(16);
        }
        if tp == 247 || tp == 248 {
            table.Columns[0]
                .FieldType
                .SetElems(vec!["z".into(), "a".into()]);
        }
        let source = rows(&values);
        let partition = if tp == 16 {
            let builder = astersql_statistics::RuntimeStatsBuilder::default();
            let mut h =
                astersql_statistics::NewHistogram(1, 3, 0, 1, &table.Columns[0].FieldType, 3, 0);
            for (i, v) in [1, 256, 2].into_iter().enumerate() {
                let mut d = datum::Datum::default();
                d.SetMysqlBit(datum::NewBinaryLiteralFromUint(v, 2));
                h.AppendBucket(&d, &d, i as i64 + 1, 1);
            }
            let mut p = TableStats::default();
            p.columns.insert(
                1,
                crate::ColumnStats {
                    analyzed_or_synthesized: true,
                    ndv: 3,
                    buckets: h
                        .Buckets
                        .iter()
                        .enumerate()
                        .map(|(i, b)| crate::Bucket {
                            count: b.Count,
                            repeats: b.Repeat,
                            ndv: 0,
                            lower: builder.encode_histogram_bound(&h, i * 2, false).unwrap(),
                            upper: builder
                                .encode_histogram_bound(&h, i * 2 + 1, false)
                                .unwrap(),
                        })
                        .collect(),
                    ..Default::default()
                },
            );
            p
        } else {
            BuildRuntimeTableStats(11, &table, &source, 1, 0).unwrap()
        };
        let mut global = partition.clone();
        crate::MergeRuntimePartitionStats(
            &astersql_statistics::RuntimeStatsBuilder::default(),
            &table,
            &mut global,
            &[partition],
            1,
            2,
            &sqlkiller::sqlkiller::SQLKiller::default(),
        )
        .unwrap();
        let top = global.columns[&1]
            .top_n
            .iter()
            .map(|e| e.1 as i64)
            .sum::<i64>();
        assert_eq!(column_histogram_rows(&global, 1) + top, 3, "type {tp}");
    }
}
