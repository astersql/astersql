// Copyright 2026 AsterSQL.

use crate::restore_scan_rows_before_residual_filter;

#[test]
fn residual_selection_restores_its_scan_input_cardinality() {
    let scan_rows = restore_scan_rows_before_residual_filter(
        5_864_745.23,
        5_864_745.23 / 12_041_103.35,
        12_041_103.35,
    );

    assert!((scan_rows - 12_041_103.35).abs() < 1e-9);
}
