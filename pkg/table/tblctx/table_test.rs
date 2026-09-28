// Copyright 2026 AsterSQL.

use super::*;

#[derive(Default)]
struct MockTempTable {
    meta: model::TableInfo,
    size: i64,
}

impl TemporaryTable for MockTempTable {
    fn GetMeta(&self) -> &model::TableInfo {
        &self.meta
    }

    fn GetSize(&self) -> i64 {
        self.size
    }

    fn SetSize(&mut self, size: i64) {
        self.size = size;
    }
}

/// Go's `int` is 64-bit on supported production targets, so a valid delta must
/// not be narrowed to `i32` at the Rust API boundary.
#[test]
fn update_txn_delta_size_accepts_go_int_range() {
    let mut handler = NewTemporaryTableHandler(MockTempTable::default(), None);

    handler.UpdateTxnDeltaSize(i64::from(i32::MAX) + 1);

    assert_eq!(handler.GetDirtySize(), i64::from(i32::MAX) + 1);
}
