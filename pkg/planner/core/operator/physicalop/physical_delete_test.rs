// Copyright 2026 AsterSQL.

use super::{TblColPosInfo, TblColPosInfoSliceExt};

fn position(table_id: i64, start: usize, end: usize) -> TblColPosInfo {
    TblColPosInfo {
        TblID: table_id,
        Start: start,
        End: end,
        HandleCols: Vec::new(),
        IndexesRowLayout: None,
    }
}

/// Go searches for the first Start strictly greater than the ordinal, then steps back.
#[test]
fn find_tbl_idx_matches_go_strict_start_boundary() {
    let positions = vec![position(1, 0, 3), position(2, 3, 8), position(3, 8, 10)];

    assert_eq!(positions.FindTblIdx(0), Some(0));
    assert_eq!(positions.FindTblIdx(3), Some(1));
    assert_eq!(positions.FindTblIdx(8), Some(2));
    assert_eq!(positions.FindTblIdx(99), Some(2));
    assert_eq!(positions.FindTblIdx(usize::MAX), Some(2));
}

#[test]
fn find_tbl_idx_rejects_empty_and_ordinals_before_first_start() {
    assert_eq!(Vec::<TblColPosInfo>::new().FindTblIdx(0), None);

    let positions = vec![position(1, 2, 4), position(2, 7, 9)];
    assert_eq!(positions.FindTblIdx(0), None);
    assert_eq!(positions.FindTblIdx(1), None);
    assert_eq!(positions.FindTblIdx(2), Some(0));
}

#[test]
fn table_positions_compare_only_their_start_like_go() {
    assert_eq!(
        position(1, 3, 4).Cmp(&position(99, 3, 100)),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        position(1, 2, 100).Cmp(&position(1, 3, 4)),
        std::cmp::Ordering::Less
    );
}
