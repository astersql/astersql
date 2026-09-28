// Copyright 2026 AsterSQL.

use super::*;
use crate::chunk_executor_kernel::VectorizedFilterConsiderNull;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[test]
fn row_filter_restores_chunk_selection_after_evaluation_error() {
    let ctx = exprstatic::NewEvalContext(Vec::new());
    let field_type = *types::NewFieldType(mysql::TypeLonglong);
    let mut input = chunk::NewChunkWithCapacity(vec![field_type.clone()], 3);
    input.AppendInt64(0, 1);
    input.AppendInt64(0, 2);
    input.AppendInt64(0, 3);
    input.SetSel(Some(vec![0, 2]));
    let mut iterator = chunk::NewIterator4Chunk(input);
    // Offset 1 is outside the one-column input and makes row evaluation unwind.
    let filters: Vec<ExprBox> = vec![Box::new(Column::new(field_type, 1, 1, 1))];

    let result = catch_unwind(AssertUnwindSafe(|| {
        VectorizedFilterConsiderNull(
            &ctx,
            false,
            &filters,
            &mut iterator,
            Vec::new(),
            Some(Vec::new()),
        )
    }));

    assert!(result.is_err());
    assert_eq!(iterator.GetChunk().Sel(), Some(&[0, 2][..]));
}
