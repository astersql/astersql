// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::time::Duration;

use astersql_planner_core_base::ContextRef;
use astersql_util_chunk as chunk;

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, Key, SchemaColumn,
};

/// Evaluates every canonical projection expression for each typed input row.
/// The evaluation context can refer to prepared parameters and session state,
/// so this operator stays bound to its original session.
pub struct TypedProjection {
    child: Box<dyn ExecExecutor>,
    expressions: Vec<astersql_expression::ExprBox>,
    context: ContextRef,
    schema: Vec<SchemaColumn>,
    no_delay: bool,
    source: chunk::Chunk,
    source_index: usize,
    source_keys: Vec<Key>,
    page_keys: Vec<Key>,
    opened: bool,
    closed: bool,
}

impl TypedProjection {
    pub fn new(
        child: Box<dyn ExecExecutor>,
        expressions: Vec<astersql_expression::ExprBox>,
        context: ContextRef,
        no_delay: bool,
    ) -> Self {
        let schema = expressions
            .iter()
            .map(|expression| SchemaColumn {
                field_type: expression
                    .GetType(context.GetExprCtx().GetEvalCtx())
                    .clone(),
            })
            .collect();
        let source = child.NewChunk();
        Self {
            child,
            expressions,
            context,
            schema,
            no_delay,
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
            return Err(astersql_errors::New("projection executor is not open"));
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
            for (column, expression) in self.expressions.iter().enumerate() {
                let datum = expression
                    .Eval(self.context.GetExprCtx().GetEvalCtx(), row.clone())
                    .map_err(|error| astersql_errors::New(error.to_string()))?;
                output.AppendDatum(column, &datum);
            }
            if self.expressions.is_empty() {
                output.SetNumVirtualRows(output.NumRows() + 1);
            }
            if !self.source_keys.is_empty() {
                self.page_keys.push(self.source_keys[index].clone());
            }
        }
        Ok(())
    }
}

impl ExecExecutor for TypedProjection {
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
        let child = self.child.ChunkConfig();
        ChunkConfig {
            fields: self
                .schema
                .iter()
                .map(|column| column.field_type.clone())
                .collect(),
            initial_capacity: child.initial_capacity,
            maximum_chunk_size: child.maximum_chunk_size,
        }
    }
    fn NewChunk(&self) -> chunk::Chunk {
        let config = self.ChunkConfig();
        *chunk::New(
            config.fields,
            config.initial_capacity,
            config.maximum_chunk_size,
        )
    }
    fn Schema(&self) -> &[SchemaColumn] {
        &self.schema
    }
    fn CalculateNoDelay(&self) -> bool {
        self.no_delay
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

/// The physical dual leaf used by constant projections and DO. Like Go's
/// TableDualExec it emits its zero or one row once, then EOF until reopened.
pub(crate) struct TypedTableDual {
    rows: usize,
    returned: bool,
    schema: Vec<SchemaColumn>,
    config: ChunkConfig,
}

impl TypedTableDual {
    pub(crate) fn new(
        rows: usize,
        schema: Vec<SchemaColumn>,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Self {
        let config = ChunkConfig {
            fields: schema
                .iter()
                .map(|column| column.field_type.clone())
                .collect(),
            initial_capacity,
            maximum_chunk_size,
        };
        Self {
            rows,
            returned: false,
            schema,
            config,
        }
    }
}

impl ExecExecutor for TypedTableDual {
    fn Open(&mut self) -> AdapterResult {
        self.returned = false;
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        Ok(())
    }
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        output.Reset();
        if self.returned || self.rows == 0 {
            return Ok(());
        }
        if self.schema.is_empty() {
            output.SetNumVirtualRows(self.rows);
        } else {
            for column in 0..self.schema.len() {
                output.AppendNull(column);
            }
        }
        self.returned = true;
        Ok(())
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        self.config.clone()
    }
    fn NewChunk(&self) -> chunk::Chunk {
        *chunk::New(
            self.config.fields.clone(),
            self.config.initial_capacity,
            self.config.maximum_chunk_size,
        )
    }
    fn Schema(&self) -> &[SchemaColumn] {
        &self.schema
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
