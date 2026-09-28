// Copyright 2026 AsterSQL.

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::legacy_vectorized_runtime::Column;

#[test]
fn column_row_access_rejects_out_of_bounds_like_go_chunk_column() {
    let mut column = Column::default();
    column.ResizeInt64(1, false);

    assert!(catch_unwind(|| column.IsNull(1)).is_err());
    assert!(catch_unwind(AssertUnwindSafe(|| column.SetNull(1, true))).is_err());
}

#[test]
fn merge_nulls_rejects_mismatched_lengths_like_go_chunk_column() {
    let mut result = Column::default();
    result.ResizeInt64(1, false);
    let mut input = Column::default();
    input.ResizeInt64(2, false);

    assert!(catch_unwind(AssertUnwindSafe(|| result.MergeNulls(&[&input]))).is_err());

    let mut strings = Column::default();
    strings.ReserveString(1);
    strings.AppendString("value".into());
    assert!(catch_unwind(AssertUnwindSafe(|| strings.MergeNulls(&[]))).is_err());
}
