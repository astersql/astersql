// Copyright 2026 AsterSQL.

use super::physical_common_plans::{Datum, PhysicalExpr};
use super::tiflash_predicate_push_down::*;
use std::collections::BTreeMap;

fn scalar(function: &str, args: Vec<PhysicalExpr>) -> PhysicalExpr {
    PhysicalExpr::Scalar {
        function: function.into(),
        args,
    }
}

fn scan(filters: Vec<PhysicalExpr>, indexes: Vec<ColumnarIndex>) -> TiFlashTableScan {
    TiFlashTableScan {
        column_ids: vec![0, 1, 2, 3],
        filters,
        realtime_count: 20_000.0,
        row_count: 20_000.0,
        keep_order: false,
        indexes,
        used_indexes: Vec::new(),
        late_materialization: false,
        late_filter_conditions: Vec::new(),
        late_selectivity: 1.0,
        expression_selectivity: BTreeMap::new(),
        full_text_push_down: false,
    }
}

#[test]
fn late_materialization_selects_large_unordered_scan_filters() {
    let lower = scalar(
        "ge",
        vec![
            PhysicalExpr::Column(2),
            PhysicalExpr::Constant(Datum::Int(1)),
        ],
    );
    let upper = scalar(
        "lt",
        vec![
            PhysicalExpr::Column(2),
            PhysicalExpr::Constant(Datum::Int(2)),
        ],
    );
    let mut scan = scan(vec![lower.clone(), upper.clone()], Vec::new());
    scan.late_materialization = true;
    scan.expression_selectivity
        .insert(format!("{lower:?}"), 0.2);
    scan.expression_selectivity
        .insert(format!("{upper:?}"), 0.2);

    handle_tiflash_predicate_push_down(&mut scan, &[]).unwrap();

    assert_eq!(scan.late_filter_conditions, vec![lower, upper]);
    assert!((scan.late_selectivity - 0.04).abs() < f64::EPSILON);
    assert!((scan.row_count - 800.0).abs() < 1e-9);
}

#[test]
fn late_materialization_respects_pack_size_order_and_filter_gates() {
    let filter = scalar(
        "ge",
        vec![
            PhysicalExpr::Column(2),
            PhysicalExpr::Constant(Datum::Int(1)),
        ],
    );
    let mut small = scan(vec![filter.clone()], Vec::new());
    small.late_materialization = true;
    small.realtime_count = 8192.0;
    handle_tiflash_predicate_push_down(&mut small, &[]).unwrap();
    assert!(small.late_filter_conditions.is_empty());

    let mut ordered = scan(vec![filter], Vec::new());
    ordered.late_materialization = true;
    ordered.keep_order = true;
    handle_tiflash_predicate_push_down(&mut ordered, &[]).unwrap();
    assert!(ordered.late_filter_conditions.is_empty());

    let mut unfiltered = scan(Vec::new(), Vec::new());
    unfiltered.late_materialization = true;
    handle_tiflash_predicate_push_down(&mut unfiltered, &[]).unwrap();
    assert!(unfiltered.late_filter_conditions.is_empty());
}

#[test]
fn heavy_cost_function_set_matches_go_switch() {
    for function in [
        "json_array_append",
        "json_contains_path",
        "json_depth",
        "json_storage_size",
        "add_time",
        "date_sub",
        "day_of_year",
        "timestamp_add",
        "period_diff",
        "time_to_sec",
    ] {
        assert!(
            with_heavy_cost_function(&scalar(function, vec![])),
            "{function} must be treated as heavy"
        );
    }
}

#[test]
fn later_inverted_index_replaces_earlier_index_for_same_column() {
    let filter = scalar("eq", vec![PhysicalExpr::Column(1)]);
    let indexes = vec![
        ColumnarIndex {
            id: 10,
            name: "old".into(),
            column_ids: vec![1],
            kind: ColumnarIndexKind::Inverted,
            public: true,
        },
        ColumnarIndex {
            id: 20,
            name: "new".into(),
            column_ids: vec![1],
            kind: ColumnarIndexKind::Inverted,
            public: true,
        },
    ];
    let mut scan = scan(vec![filter.clone()], indexes);
    scan.expression_selectivity
        .insert(format!("{filter:?}"), 0.5);

    handle_tiflash_predicate_push_down(&mut scan, &[]).unwrap();

    assert_eq!(
        scan.used_indexes
            .iter()
            .map(|index| index.id)
            .collect::<Vec<_>>(),
        vec![20]
    );
}

#[test]
fn existing_inverted_index_is_appended_again_like_go() {
    let filter = scalar("eq", vec![PhysicalExpr::Column(1)]);
    let index = ColumnarIndex {
        id: 10,
        name: "idx".into(),
        column_ids: vec![1],
        kind: ColumnarIndexKind::Inverted,
        public: true,
    };
    let mut scan = scan(vec![filter.clone()], vec![index.clone()]);
    scan.used_indexes.push(index);
    scan.expression_selectivity
        .insert(format!("{filter:?}"), 0.5);

    handle_tiflash_predicate_push_down(&mut scan, &[]).unwrap();

    assert_eq!(
        scan.used_indexes
            .iter()
            .map(|index| index.id)
            .collect::<Vec<_>>(),
        vec![10, 10]
    );
}
