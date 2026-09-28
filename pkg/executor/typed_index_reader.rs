// Copyright 2026 AsterSQL.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_meta_model::ColumnInfo;
use astersql_parser_mysql::r#type::HasPriKeyFlag;
use astersql_util_chunk as chunk;

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, Key, SchemaColumn,
};
use crate::typed_kv_scan::KeyRange;

/// Covered index read: decodes stored index datums without fetching a table
/// record, while deriving record locking keys from the index handle.
pub struct TypedIndexReader {
    retriever: Arc<dyn kv::Retriever + Send + Sync>,
    table_id: i64,
    index_column_ids: Vec<i64>,
    output_columns: Vec<ColumnInfo>,
    schema: Vec<SchemaColumn>,
    descending: bool,
    ranges: Vec<KeyRange>,
    range_index: usize,
    cursor: Option<kv::Key>,
    page_keys: Vec<Key>,
    scanned_rows: usize,
    initial_capacity: usize,
    maximum_chunk_size: usize,
    opened: bool,
    closed: bool,
}

impl TypedIndexReader {
    pub fn new(
        retriever: Arc<dyn kv::Retriever + Send + Sync>,
        table_id: i64,
        index_column_ids: Vec<i64>,
        output_columns: Vec<ColumnInfo>,
        descending: bool,
        mut ranges: Vec<KeyRange>,
        initial_capacity: usize,
        maximum_chunk_size: usize,
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
        let schema = output_columns
            .iter()
            .map(|column| SchemaColumn {
                field_type: column.FieldType.clone(),
            })
            .collect();
        Self {
            retriever,
            table_id,
            index_column_ids,
            output_columns,
            schema,
            descending,
            ranges,
            range_index: 0,
            cursor: None,
            page_keys: Vec::new(),
            scanned_rows: 0,
            initial_capacity,
            maximum_chunk_size,
            opened: false,
            closed: false,
        }
    }

    fn append_index_row(
        &self,
        key: kv::Key,
        value: Vec<u8>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult<Key> {
        let (encoded, _) = astersql_tablecodec::CutIndexKey(
            astersql_tablecodec::kv::Key(key.0.clone()),
            self.index_column_ids.clone(),
        )?;
        let handle =
            astersql_tablecodec::DecodeIndexHandle(key.0, value, self.index_column_ids.len())?
                .ok_or_else(|| errors::New("index KV has no row handle"))?;
        let (record_table_id, record_handle) =
            if let Some(partition) = handle.as_any().downcast_ref::<kv::PartitionHandle>() {
                (partition.PartitionID, partition.Handle.Copy())
            } else {
                (self.table_id, handle.Copy())
            };
        let record_key =
            astersql_tablecodec::EncodeRowKeyWithHandle(record_table_id, record_handle);
        let mut datums = HashMap::new();
        for (id, bytes) in encoded {
            let (_, datum) = astersql_util_codec::DecodeOne(&bytes)?;
            datums.insert(id, datum);
        }
        for (offset, column) in self.output_columns.iter().enumerate() {
            let datum = if let Some(datum) = datums.remove(&column.ID) {
                datum
            } else if HasPriKeyFlag(column.GetFlag()) && handle.IsInt() {
                astersql_types::datum::NewIntDatum(handle.IntValue())
            } else {
                return Err(errors::New(format!(
                    "column {} is not covered by index",
                    column.Name.O
                )));
            };
            output.AppendDatum(offset, &datum);
        }
        if self.output_columns.is_empty() {
            output.SetNumVirtualRows(output.NumRows() + 1);
        }
        Ok(record_key.0)
    }

    fn next_inner(
        &mut self,
        context: Option<&ExecutionContext>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        if !self.opened || self.closed {
            return Err(errors::New("index reader executor is not open"));
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
                    let key = iterator.Key();
                    let record_key =
                        self.append_index_row(key.clone(), iterator.Value(), output)?;
                    self.scanned_rows += 1;
                    self.page_keys.push(record_key);
                    if self.descending {
                        self.cursor = Some(key);
                    } else {
                        let mut next = key.0;
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

impl ExecExecutor for TypedIndexReader {
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
        ChunkConfig {
            fields: self
                .schema
                .iter()
                .map(|column| column.field_type.clone())
                .collect(),
            initial_capacity: self.initial_capacity,
            maximum_chunk_size: self.maximum_chunk_size,
        }
    }
    fn NewChunk(&self) -> chunk::Chunk {
        *chunk::New(
            self.output_columns
                .iter()
                .map(|column| column.FieldType.clone())
                .collect::<Vec<_>>(),
            self.initial_capacity,
            self.maximum_chunk_size,
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
    fn TakeLockKeys(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.page_keys)
    }
    fn ScannedRows(&self) -> usize {
        self.scanned_rows
    }
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        Some(Box::new(Self {
            retriever: self.retriever.clone(),
            table_id: self.table_id,
            index_column_ids: self.index_column_ids.clone(),
            output_columns: self.output_columns.clone(),
            schema: self.schema.clone(),
            descending: self.descending,
            ranges: self.ranges.clone(),
            range_index: self.range_index,
            cursor: self.cursor.clone(),
            page_keys: Vec::new(),
            scanned_rows: self.scanned_rows,
            initial_capacity: self.initial_capacity,
            maximum_chunk_size: self.maximum_chunk_size,
            opened: self.opened,
            closed: self.closed,
        }))
    }
}
