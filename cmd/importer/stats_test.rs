// Copyright 2026 AsterSQL.

use crate::stats::{getValidPrefix, histogram};
use crate::stubs::{Bounds, HistogramCore, seed_rng};
use crate::{parser, stats};

#[test]
#[should_panic]
fn empty_bounds_preserve_go_divide_by_zero_failure() {
    let hist = histogram::from_core(
        HistogramCore {
            Bounds: Bounds::default(),
            ..HistogramCore::default()
        },
        None,
    );

    let _ = hist.getAvgLen(32);
}

#[test]
fn valid_prefix_visits_go_range_byte_offsets_only() {
    for seed in 0..32 {
        seed_rng(seed);
        assert_eq!(getValidPrefix("é", "ÿ"), "é");
    }
}

#[test]
fn bundled_tidb_stats_json_restores_named_histograms() {
    let mut table = parser::newTable();
    parser::parseTableSQL(
        &mut table,
        "create table t(a int primary key, b double, c varchar(10), d date unique);",
    )
    .unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("stats.json");

    let loaded = stats::loadStats(&table.tblInfo, path.to_str().unwrap()).unwrap();
    let integer = loaded.GetCol(table.tblInfo.Columns[0].ID).unwrap();
    assert_eq!(integer.Histogram.Buckets.len(), 157);
    assert_eq!(integer.Histogram.Bounds.GetRow(0).GetInt64(0), 0);
    assert_eq!(integer.Histogram.Bounds.GetRow(1).GetInt64(0), 63);

    let date_index = table
        .tblInfo
        .Indices
        .iter()
        .find(|index| index.Columns[0].Name == "d")
        .unwrap();
    let date = loaded.GetIdx(date_index.ID).unwrap();
    assert!(!date.Histogram.Buckets.is_empty());
    assert_eq!(
        date.Histogram.Bounds.GetRow(0).GetTime(0).format_date(),
        "2018-01-31"
    );
}
