// Copyright 2026 AsterSQL.

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_types::datum::Datum;
use astersql_util_chunk as chunk;
use astersql_util_logutil_consistency as consistency;

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, Key, SchemaColumn,
};
use crate::typed_kv_scan::TypedKVScan;

/// Canonical session lock boundary for point reads. RC returns the current
/// value only after locking an existing key; RR locks even missing keys before
/// the pinned snapshot read.
pub trait PointLockRuntime {
    fn IsReadCommitted(&self) -> bool;
    fn LocalValue(&self, _key: kv::Key) -> AdapterResult<Option<Option<Vec<u8>>>> {
        Ok(None)
    }
    fn LockKey(
        &self,
        key: kv::Key,
        only_if_exists: bool,
        wait_ms: i64,
    ) -> AdapterResult<Option<Vec<u8>>>;
}

/// An owned snapshot point reader. The index path resolves its row handle
/// before fetching the record, while a handle plan fetches only the record.
pub struct TypedPointGet {
    retriever: Arc<dyn kv::Retriever + Send + Sync>,
    decoder: TypedKVScan,
    logical_table_id: i64,
    physical_table_id: i64,
    handle: Option<i64>,
    index_id: Option<i64>,
    index_table: Option<astersql_meta_model::TableInfo>,
    index_info: Option<astersql_meta_model::IndexInfo>,
    index_values: Vec<Datum>,
    index_columns: usize,
    consistency_logger: Arc<dyn consistency::LogSink>,
    consistency_storage: Option<Arc<dyn consistency::Storage>>,
    redact_mode: String,
    weak_consistency: bool,
    lock: bool,
    lock_wait_ms: i64,
    lock_runtime: Option<Rc<dyn PointLockRuntime>>,
    table_dual: bool,
    done: bool,
    opened: bool,
    closed: bool,
    page_keys: Vec<Key>,
    scanned_rows: usize,
}

impl TypedPointGet {
    pub fn WithIndexMetadata(
        mut self,
        table: astersql_meta_model::TableInfo,
        index: astersql_meta_model::IndexInfo,
    ) -> Self {
        self.index_table = Some(table);
        self.index_info = Some(index);
        self
    }
    pub fn WithConsistencyDiagnostics(
        mut self,
        logger: Arc<dyn consistency::LogSink>,
        storage: Option<Arc<dyn consistency::Storage>>,
        redact_mode: String,
    ) -> Self {
        self.consistency_logger = logger;
        self.consistency_storage = storage;
        self.redact_mode = redact_mode;
        self
    }
    pub fn SetDiagnosticMode(&mut self, weak_consistency: bool, redact_mode: String) {
        self.weak_consistency = weak_consistency;
        self.redact_mode = redact_mode;
    }
    pub fn WithLockPlan(mut self, lock: bool, wait_ms: i64) -> Self {
        self.lock = lock;
        self.lock_wait_ms = wait_ms;
        self
    }
    pub fn SetLockRuntime(&mut self, runtime: Rc<dyn PointLockRuntime>) {
        self.lock_runtime = Some(runtime);
    }
    pub fn IsReusable(&self) -> bool {
        !self.opened || self.closed
    }

    pub fn RecreatedFromPlan(
        &mut self,
        plan: &astersql_planner_core_operator_physicalop::PointGetPlan,
    ) -> AdapterResult {
        if !self.IsReusable() {
            return Err(errors::New("PointGet executor is still open"));
        }
        let table = plan
            .TblInfo
            .as_ref()
            .ok_or_else(|| errors::New("PointGet has no TableInfo"))?;
        if table.ID != self.logical_table_id
            || plan.PartitionIdx.is_some()
            || plan.Lock
            || plan.IndexInfo.as_ref().map(|index| index.ID) != self.index_id
            || plan
                .Columns
                .iter()
                .map(|column| column.ID)
                .collect::<Vec<_>>()
                != self.decoder.ColumnIDs()
            || (self.index_id.is_some() && plan.IndexValues.len() != self.index_columns)
        {
            return Err(errors::New("cached PointGet shape changed"));
        }
        self.handle = plan.Handle;
        self.index_values = plan.IndexValues.clone();
        self.table_dual = plan.IsTableDual;
        self.done = false;
        self.opened = false;
        self.closed = false;
        self.page_keys.clear();
        self.scanned_rows = 0;
        Ok(())
    }

    pub fn new(
        retriever: Arc<dyn kv::Retriever + Send + Sync>,
        logical_table_id: i64,
        physical_table_id: i64,
        pk_is_handle: bool,
        columns: Vec<astersql_meta_model::ColumnInfo>,
        handle: Option<i64>,
        index_id: Option<i64>,
        index_values: Vec<Datum>,
        index_columns: usize,
        table_dual: bool,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Self {
        let decoder = TypedKVScan::new(
            retriever.clone(),
            physical_table_id,
            pk_is_handle,
            false,
            columns,
            Vec::new(),
            initial_capacity,
            maximum_chunk_size,
        );
        Self {
            retriever,
            decoder,
            logical_table_id,
            physical_table_id,
            handle,
            index_id,
            index_table: None,
            index_info: None,
            index_values,
            index_columns,
            consistency_logger: Arc::new(consistency::StandardLogSink),
            consistency_storage: None,
            redact_mode: "OFF".into(),
            weak_consistency: false,
            lock: false,
            lock_wait_ms: 0,
            lock_runtime: None,
            table_dual,
            done: false,
            opened: false,
            closed: false,
            page_keys: Vec::new(),
            scanned_rows: 0,
        }
    }

    fn get_optional(&self, key: kv::Key) -> AdapterResult<Option<Vec<u8>>> {
        match self.retriever.Get(&kv::Context::default(), key, &[]) {
            Ok(value) => Ok(Some(value.Value)),
            Err(error) if kv::ErrNotExist.Equal(Some(&error)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn get_and_lock_optional(&self, key: kv::Key) -> AdapterResult<Option<Vec<u8>>> {
        if !self.lock {
            if let Some(runtime) = &self.lock_runtime {
                if let Some(value) = runtime.LocalValue(key.clone())? {
                    return Ok(value);
                }
            }
            return self.get_optional(key);
        }
        let runtime = self
            .lock_runtime
            .as_ref()
            .ok_or_else(|| errors::New("locking PointGet has no canonical point lock runtime"))?;
        if runtime.IsReadCommitted() {
            runtime.LockKey(key, true, self.lock_wait_ms)
        } else {
            runtime.LockKey(key.clone(), false, self.lock_wait_ms)?;
            if let Some(value) = runtime.LocalValue(key.clone())? {
                return Ok(value);
            }
            self.get_optional(key)
        }
    }

    fn next_inner(
        &mut self,
        context: Option<&ExecutionContext>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        if !self.opened || self.closed {
            return Err(errors::New("point get executor is not open"));
        }
        output.Reset();
        self.page_keys.clear();
        if self.done {
            return Ok(());
        }
        let check_kill = || -> AdapterResult {
            if let Some(killer) = context.and_then(|context| context.sql_killer.as_ref()) {
                killer.HandleSignal()?;
            }
            Ok(())
        };
        check_kill()?;
        self.done = true;
        if self.table_dual {
            return Ok(());
        }
        let mut fetched_index_key = None;
        let (table_id, handle) = if let Some(index_id) = self.index_id {
            let index_key = if let (Some(table), Some(index)) =
                (self.index_table.as_ref(), self.index_info.as_ref())
            {
                let index_table_id = if index.Global {
                    self.logical_table_id
                } else {
                    self.physical_table_id
                };
                let (key, distinct) = astersql_tablecodec::GenIndexKey(
                    astersql_tablecodec::codec::NewEncoder(
                        astersql_tablecodec::collate::NewCollationEnabled(),
                    ),
                    Some(astersql_tablecodec::time::UTC),
                    Box::new(table.clone()),
                    Box::new(index.clone()),
                    index_table_id,
                    self.index_values.clone(),
                    None,
                    None,
                )?;
                if !distinct {
                    return Ok(());
                }
                kv::Key(key)
            } else {
                let encoded = astersql_util_codec::EncodeKey(
                    astersql_tablecodec::time::UTC,
                    Vec::new(),
                    self.index_values.clone(),
                )?;
                kv::Key(
                    astersql_tablecodec::EncodeIndexSeekKey(
                        self.logical_table_id,
                        index_id,
                        Some(encoded),
                    )
                    .0,
                )
            };
            let Some(value) = self.get_and_lock_optional(kv::Key(index_key.0.clone()))? else {
                return Ok(());
            };
            fetched_index_key = Some(index_key.clone());
            check_kill()?;
            let decoded =
                astersql_tablecodec::DecodeIndexHandle(index_key.0, value, self.index_columns)?
                    .ok_or_else(|| errors::New("point get index KV has no row handle"))?;
            if let Some(partition) = decoded.as_any().downcast_ref::<kv::PartitionHandle>() {
                (partition.PartitionID, partition.Handle.Copy())
            } else {
                (self.physical_table_id, decoded)
            }
        } else {
            let handle = self
                .handle
                .ok_or_else(|| errors::New("point get plan has no row handle"))?;
            (
                self.physical_table_id,
                Box::new(kv::IntHandle(handle)) as Box<dyn kv::Handle>,
            )
        };
        let diagnostic_handle = handle.Copy();
        let key = astersql_tablecodec::EncodeRowKeyWithHandle(table_id, handle);
        let Some(record) = self.get_and_lock_optional(kv::Key(key.0.clone()))? else {
            if let Some(index_id) = self.index_id {
                if self.weak_consistency {
                    return Ok(());
                }
                if let (Some(table), Some(index), Some(index_key)) = (
                    self.index_table.as_ref(),
                    self.index_info.as_ref(),
                    fetched_index_key,
                ) {
                    let row_key_table_id = table_id;
                    let reporter = consistency::Reporter::new(
                        Arc::new(move |handle| {
                            astersql_tablecodec::EncodeRowKeyWithHandle(
                                row_key_table_id,
                                handle.Copy(),
                            )
                        }),
                        Arc::new(move |_row| index_key.clone()),
                        table.clone(),
                        index.clone(),
                        self.redact_mode.clone(),
                        self.consistency_storage.clone(),
                        self.consistency_logger.clone(),
                    );
                    let missing = vec![diagnostic_handle.Copy()];
                    let complete = vec![diagnostic_handle.Copy()];
                    let values = vec![consistency::RecordData::new(
                        diagnostic_handle,
                        self.index_values.clone(),
                    )];
                    return Err(errors::New(
                        reporter
                            .ReportLookupInconsistent(1, 0, &missing, &complete, &values)
                            .to_string(),
                    ));
                }
                return Err(errors::New(format!(
                    "[executor:8133]data inconsistency in table: {}, index: {}, index-count:1 != record-count:0",
                    self.index_table.as_ref().map_or_else(
                        || self.logical_table_id.to_string(),
                        |table| table.Name.O.clone()
                    ),
                    self.index_info
                        .as_ref()
                        .map_or_else(|| index_id.to_string(), |index| index.Name.O.clone()),
                )));
            }
            return Ok(());
        };
        check_kill()?;
        self.decoder.append_decoded_row_for_table(
            kv::Key(key.0.clone()),
            record,
            table_id,
            output,
        )?;
        self.page_keys.push(key.0);
        self.scanned_rows += 1;
        Ok(())
    }
}

/// A prepared MaxTS point reader retains one executor actor across closed
/// record sets. Each record set owns only its view of the actor's output schema.
pub struct SharedTypedPointGet {
    actor: Arc<Mutex<TypedPointGet>>,
    schema: Vec<SchemaColumn>,
    config: ChunkConfig,
}

impl SharedTypedPointGet {
    pub fn new(actor: Arc<Mutex<TypedPointGet>>) -> Self {
        let (schema, config) = {
            let point = actor.lock().expect("PointGet actor lock poisoned");
            (point.Schema().to_vec(), point.ChunkConfig())
        };
        Self {
            actor,
            schema,
            config,
        }
    }
}

impl ExecExecutor for SharedTypedPointGet {
    fn Open(&mut self) -> AdapterResult {
        self.actor
            .lock()
            .expect("PointGet actor lock poisoned")
            .Open()
    }
    fn Close(&mut self) -> AdapterResult {
        self.actor
            .lock()
            .expect("PointGet actor lock poisoned")
            .Close()
    }
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        self.actor
            .lock()
            .expect("PointGet actor lock poisoned")
            .Next(output)
    }
    fn NextWithContext(
        &mut self,
        context: &ExecutionContext,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        self.actor
            .lock()
            .expect("PointGet actor lock poisoned")
            .NextWithContext(context, output)
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
    fn TakeLockKeys(&mut self) -> Vec<Key> {
        self.actor
            .lock()
            .expect("PointGet actor lock poisoned")
            .TakeLockKeys()
    }
    fn ScannedRows(&self) -> usize {
        self.actor
            .lock()
            .expect("PointGet actor lock poisoned")
            .ScannedRows()
    }
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        self.actor
            .lock()
            .expect("PointGet actor lock poisoned")
            .Detach()
    }
}

impl ExecExecutor for TypedPointGet {
    fn Open(&mut self) -> AdapterResult {
        if self.lock && self.lock_runtime.is_none() {
            return Err(errors::New(
                "locking PointGet has no canonical point lock runtime",
            ));
        }
        self.done = false;
        self.opened = true;
        self.closed = false;
        self.page_keys.clear();
        self.scanned_rows = 0;
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
        self.decoder.ChunkConfig()
    }
    fn NewChunk(&self) -> chunk::Chunk {
        self.decoder.NewChunk()
    }
    fn Schema(&self) -> &[SchemaColumn] {
        self.decoder.Schema()
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
        if self.lock {
            return None;
        }
        Some(Box::new(Self {
            retriever: self.retriever.clone(),
            decoder: self.decoder.clone_detached(),
            logical_table_id: self.logical_table_id,
            physical_table_id: self.physical_table_id,
            handle: self.handle,
            index_id: self.index_id,
            index_table: self.index_table.clone(),
            index_info: self.index_info.clone(),
            index_values: self.index_values.clone(),
            index_columns: self.index_columns,
            consistency_logger: self.consistency_logger.clone(),
            consistency_storage: self.consistency_storage.clone(),
            redact_mode: self.redact_mode.clone(),
            weak_consistency: self.weak_consistency,
            lock: false,
            lock_wait_ms: self.lock_wait_ms,
            lock_runtime: None,
            table_dual: self.table_dual,
            done: self.done,
            opened: self.opened,
            closed: self.closed,
            page_keys: Vec::new(),
            scanned_rows: self.scanned_rows,
        }))
    }
}
