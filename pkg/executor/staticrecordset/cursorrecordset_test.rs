// Copyright 2026 AsterSQL.

use std::panic::{AssertUnwindSafe, catch_unwind};

use astersql_executor_internal_exec::executor::Chunk;

use crate::{
    ChunkAllocator, CursorHandle, RecordContext, RecordSet, Result, ResultField,
    WrapRecordSetWithCursor,
};

struct TestCursor;

impl CursorHandle for TestCursor {
    fn Close(&mut self) {}
}

struct RecordSetWithoutExecutor;

impl RecordSet for RecordSetWithoutExecutor {
    fn Fields(&self) -> Vec<ResultField> {
        Vec::new()
    }

    fn Next(&mut self, _: &RecordContext, _: &mut Chunk) -> Result<()> {
        Ok(())
    }

    fn NewChunk(&self, _: Option<&dyn ChunkAllocator>) -> Chunk {
        Chunk::default()
    }

    fn Close(&mut self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn get_executor_for_test_panics_when_wrapped_recordset_has_no_executor() {
    let recordset =
        WrapRecordSetWithCursor(Box::new(TestCursor), Box::new(RecordSetWithoutExecutor));

    assert!(catch_unwind(AssertUnwindSafe(|| recordset.GetExecutor4Test())).is_err());
}
