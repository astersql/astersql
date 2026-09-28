// Copyright 2026 AsterSQL.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_meta_model::ColumnInfo;
use astersql_parser_mysql::r#type::HasPriKeyFlag;
use astersql_types::datum::NewIntDatum;
use astersql_util_chunk as chunk;

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, SchemaColumn,
};

/// A half-open encoded KV interval produced by physical table-range planning.
#[derive(Clone)]
pub struct KeyRange {
    pub start: kv::Key,
    pub end: kv::Key,
}

/// A typed, lazy table scan over the caller's real KV snapshot.
///
/// The planner owns range encoding; this executor preserves every supplied
/// interval, orders them for the scan direction, and fetches rows only when
/// Next is called. Each iterator is closed
/// before Next returns, so the executor remains Send without retaining a
/// non-Send KV iterator across calls.
pub struct TypedKVScan {
    retriever: Arc<dyn kv::Retriever + Send + Sync>,
    table_id: i64,
    pk_is_handle: bool,
    descending: bool,
    columns: Vec<ColumnInfo>,
    schema: Vec<SchemaColumn>,
    ranges: Vec<KeyRange>,
    range_index: usize,
    cursor: Option<kv::Key>,
    page_keys: Vec<kv::Key>,
    scanned_rows: usize,
    initial_capacity: usize,
    maximum_chunk_size: usize,
    opened: bool,
    closed: bool,
}

impl TypedKVScan {
    pub fn new(
        retriever: Arc<dyn kv::Retriever + Send + Sync>,
        table_id: i64,
        pk_is_handle: bool,
        descending: bool,
        columns: Vec<ColumnInfo>,
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
        let schema = columns
            .iter()
            .map(|column| SchemaColumn {
                field_type: column.FieldType.clone(),
            })
            .collect();
        Self {
            retriever,
            table_id,
            pk_is_handle,
            descending,
            columns,
            schema,
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

    pub(crate) fn append_decoded_row(
        &self,
        key: kv::Key,
        value: Vec<u8>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        self.append_decoded_row_for_table(key, value, self.table_id, output)
    }

    pub(crate) fn append_decoded_row_for_table(
        &self,
        key: kv::Key,
        value: Vec<u8>,
        expected_table_id: i64,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        let (table_id, handle) =
            astersql_tablecodec::DecodeRecordKey(astersql_tablecodec::kv::Key(key.0))?;
        if table_id != expected_table_id {
            return Err(errors::New(format!(
                "table scan key belongs to table {table_id}, expected {}",
                expected_table_id
            )));
        }
        let column_types = self
            .columns
            .iter()
            .map(|column| (column.ID, Box::new(column.FieldType.clone())))
            .collect::<HashMap<_, _>>();
        let mut decoded = astersql_tablecodec::DecodeRowToDatumMap(
            Some(value),
            column_types,
            Some(astersql_tablecodec::time::UTC),
        )?;
        if self.pk_is_handle {
            for column in &self.columns {
                if HasPriKeyFlag(column.GetFlag()) {
                    decoded
                        .entry(column.ID)
                        .or_insert_with(|| NewIntDatum(handle.IntValue()));
                }
            }
        }
        for (offset, column) in self.columns.iter().enumerate() {
            let datum = decoded.remove(&column.ID).unwrap_or_default();
            output.AppendDatum(offset, &datum);
        }
        if self.columns.is_empty() {
            output.SetNumVirtualRows(output.NumRows() + 1);
        }
        Ok(())
    }

    pub(crate) fn ColumnIDs(&self) -> Vec<i64> {
        self.columns.iter().map(|column| column.ID).collect()
    }

    pub(crate) fn clone_detached(&self) -> Self {
        Self {
            retriever: self.retriever.clone(),
            table_id: self.table_id,
            pk_is_handle: self.pk_is_handle,
            descending: self.descending,
            columns: self.columns.clone(),
            schema: self.schema.clone(),
            ranges: self.ranges.clone(),
            range_index: self.range_index,
            cursor: self.cursor.clone(),
            page_keys: Vec::new(),
            scanned_rows: self.scanned_rows,
            initial_capacity: self.initial_capacity,
            maximum_chunk_size: self.maximum_chunk_size,
            opened: self.opened,
            closed: self.closed,
        }
    }

    fn next_inner(
        &mut self,
        context: Option<&ExecutionContext>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        if !self.opened || self.closed {
            return Err(errors::New("table scan executor is not open"));
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
                    self.append_decoded_row(key.clone(), iterator.Value(), output)?;
                    self.scanned_rows += 1;
                    self.page_keys.push(key.clone());
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

impl ExecExecutor for TypedKVScan {
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
            self.columns
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
    fn TakeLockKeys(&mut self) -> Vec<crate::adapter::Key> {
        std::mem::take(&mut self.page_keys)
            .into_iter()
            .map(|key| key.0)
            .collect()
    }
    fn ScannedRows(&self) -> usize {
        self.scanned_rows
    }
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        Some(Box::new(self.clone_detached()))
    }
}
