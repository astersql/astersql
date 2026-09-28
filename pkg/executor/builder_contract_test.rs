// Copyright 2026 AsterSQL.

use std::time::Duration;

use astersql_types::field::NewFieldType;
use astersql_util_chunk as chunk;

use crate::adapter::{AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, SchemaColumn};
use crate::builder::Executor;

struct TypedLifecycleExecutor {
    opened: bool,
    closed: bool,
    emitted: bool,
}

impl ExecExecutor for TypedLifecycleExecutor {
    fn Open(&mut self) -> AdapterResult {
        self.opened = true;
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        self.closed = true;
        Ok(())
    }
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        assert!(self.opened && !self.closed);
        output.Reset();
        if !self.emitted {
            output.AppendInt64(0, 42);
            self.emitted = true;
        }
        Ok(())
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        ChunkConfig::default()
    }
    fn NewChunk(&self) -> chunk::Chunk {
        *chunk::New(vec![NewFieldType(8)], 1, 1)
    }
    fn Schema(&self) -> &[SchemaColumn] {
        &[]
    }
    fn CalculateNoDelay(&self) -> bool {
        false
    }
    fn IsWriteExecutor(&self) -> bool {
        false
    }
    fn CheckForeignKeys(&mut self) -> AdapterResult {
        Ok(())
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        Vec::new()
    }
    fn HasForeignKeyCascades(&self) -> bool {
        false
    }
    fn PrepareFKCascadeContext(&mut self) {}
    fn AddFKCheckLockDuration(&mut self, _duration: Duration) {}
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}

#[test]
fn builder_executor_exposes_real_typed_chunk_lifecycle() {
    let mut executor: Box<dyn Executor> = Box::new(TypedLifecycleExecutor {
        opened: false,
        closed: false,
        emitted: false,
    });
    executor.Open().unwrap();
    let mut output = executor.NewChunk();
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 1);
    assert_eq!(output.GetRow(0).GetInt64(0), 42);
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
    executor.Close().unwrap();
}
