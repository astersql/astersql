// Copyright 2026 AsterSQL.

use std::time::Duration;

use astersql_expression::{CNFExprs, EvalBool};
use astersql_planner_core_base::ContextRef;
use astersql_util_chunk as chunk;

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, Key, SchemaColumn,
};

/// Evaluates the canonical CNF against typed rows and retains lock keys only
/// for rows that survive the predicate. The expression context is session
/// bound, so this operator does not claim independent Detach support.
pub struct TypedSelection {
    child: Box<dyn ExecExecutor>,
    conditions: CNFExprs,
    context: ContextRef,
    source: chunk::Chunk,
    source_index: usize,
    source_keys: Vec<Key>,
    page_keys: Vec<Key>,
    opened: bool,
    closed: bool,
}

impl TypedSelection {
    pub fn new(
        child: Box<dyn ExecExecutor>,
        conditions: Vec<astersql_expression::ExprBox>,
        context: ContextRef,
    ) -> Self {
        let source = child.NewChunk();
        Self {
            child,
            conditions: CNFExprs(conditions),
            context,
            source,
            source_index: 0,
            source_keys: Vec::new(),
            page_keys: Vec::new(),
            opened: false,
            closed: false,
        }
    }

    fn next_inner(
        &mut self,
        context: Option<&ExecutionContext>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        if !self.opened || self.closed {
            return Err(astersql_errors::New("selection executor is not open"));
        }
        output.Reset();
        self.page_keys.clear();
        while !output.IsFull() {
            if self.source_index >= self.source.NumRows() {
                if let Some(context) = context {
                    self.child.NextWithContext(context, &mut self.source)?;
                } else {
                    self.child.Next(&mut self.source)?;
                }
                self.source_index = 0;
                self.source_keys = self.child.TakeLockKeys();
                if self.source.NumRows() == 0 {
                    break;
                }
                if !self.source_keys.is_empty() && self.source_keys.len() != self.source.NumRows() {
                    return Err(astersql_errors::New(
                        "lock keys do not match typed child rows",
                    ));
                }
            }
            let index = self.source_index;
            self.source_index += 1;
            let row = self.source.GetRow(index);
            let (keep, _deferred_null) = EvalBool(
                self.context.GetExprCtx().GetEvalCtx(),
                &self.conditions,
                row,
            )
            .map_err(|error| astersql_errors::New(error.to_string()))?;
            if keep {
                output.Append(&self.source, index, index + 1);
                if !self.source_keys.is_empty() {
                    self.page_keys.push(self.source_keys[index].clone());
                }
            }
        }
        Ok(())
    }
}

impl ExecExecutor for TypedSelection {
    fn Open(&mut self) -> AdapterResult {
        self.child.Open()?;
        self.source.Reset();
        self.source_index = 0;
        self.source_keys.clear();
        self.page_keys.clear();
        self.opened = true;
        self.closed = false;
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.child.Close()
    }
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        self.next_inner(None, output)
    }
    fn NextWithContext(
        &mut self,
        context: &ExecutionContext,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        self.next_inner(Some(context), output)
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        self.child.ChunkConfig()
    }
    fn NewChunk(&self) -> chunk::Chunk {
        self.child.NewChunk()
    }
    fn Schema(&self) -> &[SchemaColumn] {
        self.child.Schema()
    }
    fn CalculateNoDelay(&self) -> bool {
        false
    }
    fn IsWriteExecutor(&self) -> bool {
        false
    }
    fn CheckForeignKeys(&mut self) -> AdapterResult {
        self.child.CheckForeignKeys()
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        self.child.TakeForeignKeyCascades()
    }
    fn HasForeignKeyCascades(&self) -> bool {
        self.child.HasForeignKeyCascades()
    }
    fn PrepareFKCascadeContext(&mut self) {
        self.child.PrepareFKCascadeContext()
    }
    fn AddFKCheckLockDuration(&mut self, duration: Duration) {
        self.child.AddFKCheckLockDuration(duration)
    }
    fn TakeLockKeys(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.page_keys)
    }
    fn ScannedRows(&self) -> usize {
        self.child.ScannedRows()
    }
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}
