// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::time::Duration;

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_util_chunk as chunk;

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, Key, SchemaColumn,
};
use crate::typed_kv_scan::{KeyRange, TypedKVScan};

/// Lazy double read: index KV order determines row order, and every returned
/// row carries its encoded record key for canonical SELECT FOR UPDATE locking.
pub struct TypedIndexLookUp {
    retriever: Arc<dyn kv::Retriever + Send + Sync>,
    record_decoder: TypedKVScan,
    index_columns: usize,
    table_id: i64,
    descending: bool,
    ranges: Vec<KeyRange>,
    range_index: usize,
    cursor: Option<kv::Key>,
    page_keys: Vec<Key>,
    scanned_rows: usize,
    opened: bool,
    closed: bool,
}

impl TypedIndexLookUp {
    pub fn new(
        retriever: Arc<dyn kv::Retriever + Send + Sync>,
        record_decoder: TypedKVScan,
        index_columns: usize,
        table_id: i64,
        descending: bool,
        mut ranges: Vec<KeyRange>,
    ) -> Self {
        ranges.sort_by(|left, right| {
            if descending {
                right
                    .end
                    .0
                    .cmp(&left.end.0)
                    .then_with(|| right.start.0.cmp(&left.start.0))
            } else {
                left.start
                    .0
                    .cmp(&right.start.0)
                    .then_with(|| left.end.0.cmp(&right.end.0))
            }
        });
        Self {
            retriever,
            record_decoder,
            index_columns,
            table_id,
            descending,
            ranges,
            range_index: 0,
            cursor: None,
            page_keys: Vec::new(),
            scanned_rows: 0,
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
            return Err(errors::New("index lookup executor is not open"));
        }
        output.Reset();
        self.page_keys.clear();
        let check_kill = || -> AdapterResult {
            if let Some(killer) = context.and_then(|context| context.sql_killer.as_ref()) {
                killer.HandleSignal()?;
            }
            Ok(())
        };
        check_kill()?;
        while !output.IsFull() && self.range_index < self.ranges.len() {
            check_kill()?;
            let range = self.ranges[self.range_index].clone();
            let mut iterator = if self.descending {
                self.retriever.IterReverse(
                    Some(self.cursor.clone().unwrap_or(range.end.clone())),
                    Some(range.start.clone()),
                )?
            } else {
                self.retriever.Iter(
                    self.cursor.clone().unwrap_or(range.start.clone()),
                    Some(range.end.clone()),
                )?
            };
            let result = (|| -> AdapterResult {
                while iterator.Valid() && !output.IsFull() {
                    check_kill()?;
                    let index_key = iterator.Key();
                    let index_value = iterator.Value();
                    let handle = astersql_tablecodec::DecodeIndexHandle(
                        index_key.0.clone(),
                        index_value,
                        self.index_columns,
                    )?
                    .ok_or_else(|| errors::New("index KV has no row handle"))?;
                    let (record_table_id, record_handle) = if let Some(partition) =
                        handle.as_any().downcast_ref::<kv::PartitionHandle>()
                    {
                        (partition.PartitionID, partition.Handle.Copy())
                    } else {
                        (self.table_id, handle)
                    };
                    let record_key =
                        astersql_tablecodec::EncodeRowKeyWithHandle(record_table_id, record_handle);
                    let record =
                        self.retriever
                            .Get(&kv::Context::default(), record_key.clone(), &[])?;
                    self.record_decoder.append_decoded_row_for_table(
                        record_key.clone(),
                        record.Value,
                        record_table_id,
                        output,
                    )?;
                    self.scanned_rows += 1;
                    self.page_keys.push(record_key.0);
                    if self.descending {
                        self.cursor = Some(index_key);
                    } else {
                        let mut next = index_key.0;
                        next.push(0);
                        self.cursor = Some(kv::Key(next));
                    }
                    if !output.IsFull() {
                        iterator.Next()?;
                    }
                }
                if !iterator.Valid() {
                    self.range_index += 1;
                    self.cursor = None;
                }
                Ok(())
            })();
            iterator.Close();
            if let Err(error) = result {
                output.Reset();
                self.page_keys.clear();
                return Err(error);
            }
        }
        if let Err(error) = check_kill() {
            output.Reset();
            self.page_keys.clear();
            return Err(error);
        }
        Ok(())
    }
}

impl ExecExecutor for TypedIndexLookUp {
    fn Open(&mut self) -> AdapterResult {
        self.range_index = 0;
        self.cursor = None;
        self.page_keys.clear();
        self.scanned_rows = 0;
        self.opened = true;
        self.closed = false;
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        self.closed = true;
        Ok(())
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
        self.record_decoder.ChunkConfig()
    }
    fn NewChunk(&self) -> chunk::Chunk {
        self.record_decoder.NewChunk()
    }
    fn Schema(&self) -> &[SchemaColumn] {
        self.record_decoder.Schema()
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
    fn TakeLockKeys(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.page_keys)
    }
    fn ScannedRows(&self) -> usize {
        self.scanned_rows
    }
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        Some(Box::new(Self {
            retriever: self.retriever.clone(),
            record_decoder: self.record_decoder.clone_detached(),
            index_columns: self.index_columns,
            table_id: self.table_id,
            descending: self.descending,
            ranges: self.ranges.clone(),
            range_index: self.range_index,
            cursor: self.cursor.clone(),
            page_keys: Vec::new(),
            scanned_rows: self.scanned_rows,
            opened: self.opened,
            closed: self.closed,
        }))
    }
}
