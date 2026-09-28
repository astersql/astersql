// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// IMPORT INTO 的编码与排序（encode-and-sort）算子。
//
// 该步骤将源数据块编码为 KV（键值对），并写入 Lightning EngineWriter
// 做本地/云端排序。本文件定义算子运行时状态、chunk worker，以及
// writer 内存预算分摊与对象存储子任务前缀。

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use astersql_dxf_framework_proto::subtask::StepResource;
use astersql_dxf_framework_taskexecutor_execute::Collector;
use astersql_dxf_operator::compose::{
    DataChannel, NewSimpleDataChannel, SimpleDataChannel, WithSource,
};
use astersql_dxf_operator::operator::Operator;
use astersql_errors as errors;
use astersql_executor_importer as importer;
use astersql_executor_importer::RoutedIndexWriter;
use astersql_ingestor_simplesst::writer::WriterSink;
use astersql_ingestor_simplesst::writer::{DuplicateMode, Writer, WriterBuilder, WriterSummary};
use astersql_lightning_backend::{BackendError, ChunkFlushStatus, EngineWriter};
use astersql_lightning_backend_encode::Context;
use astersql_lightning_backend_encode::Rows;
use astersql_lightning_backend_kv::Rows2KvPairs;
use astersql_objstore_storeapi::{Context as StoreContext, StorageRef};
use astersql_resourcemanager_pool_workerpool::{
    Channel, Context as PoolContext, Error as PoolError,
};

use crate::proto::{SharedVars, SortedKVMeta, importStepMinimalTask};

fn protocol_writer_summary(summary: &WriterSummary) -> crate::proto::WriterSummary {
    crate::proto::WriterSummary {
        Min: (!summary.Min.is_empty()).then(|| summary.Min.clone()),
        Max: (!summary.Max.is_empty()).then(|| summary.Max.clone()),
        TotalSize: summary.TotalSize,
        TotalCnt: summary.TotalCnt,
        MultipleFilesStats: summary
            .MultipleFilesStats
            .iter()
            .map(|stat| crate::proto::MultipleFilesStat {
                MinKey: stat.MinKey.clone(),
                MaxKey: stat.MaxKey.clone(),
                Filenames: stat.Filenames.clone(),
                MaxOverlappingNum: stat.MaxOverlappingNum,
            })
            .collect(),
        ConflictInfo: astersql_ingestor_engineapi::ConflictInfo {
            Count: summary.ConflictInfo.Count,
            Files: summary.ConflictInfo.Files.clone(),
        },
    }
}

/// Send-only chunk envelope. Worker-local importer/AST state is never moved.
pub struct EncodeSortTask {
    pub Plan: importer::Plan,
    pub Chunk: importer::Chunk,
}

/// Built inside a worker thread so implementations may own non-Send importer state.
pub trait EncodeSortWorker {
    fn HandleTask(&mut self, task: EncodeSortTask) -> Result<(), String>;
    fn Close(&mut self) -> Result<(), String>;
}

pub type EncodeSortWorkerFactory =
    Arc<dyn Fn() -> Result<Box<dyn EncodeSortWorker>, String> + Send + Sync>;

/// Real worker state is created inside its owning thread by the configured
/// factory. This moves SharedVars between chunk calls within that same thread,
/// while the channel carries only Plan and Chunk.
pub struct ThreadOwnedEncodeSortWorker {
    worker: chunkWorker,
    shared: Option<SharedVars>,
    logger: astersql_lightning_log::Logger,
    summaries: Arc<GlobalWriterSummaries>,
}

/// Host services retained across worker creation; AST, importer and writer state
/// are reconstructed inside each worker rather than transferred across threads.
pub struct ConfiguredEncodeSortRuntime {
    pub ControllerServices: Arc<dyn Fn() -> importer::LoadDataControllerServices + Send + Sync>,
    pub ImporterService: Arc<dyn Fn() -> Arc<dyn importer::TableImporterService> + Send + Sync>,
    pub SharedImporterService: Arc<std::sync::OnceLock<Arc<dyn importer::TableImporterService>>>,
    pub ObjectStore: StorageRef,
    /// Creates a per-subtask cloud store that the step executor closes after use.
    pub ObjectStoreFactory: Option<Arc<dyn Fn() -> Result<StorageRef, String> + Send + Sync>>,
    pub LoggerFactory: Arc<dyn Fn() -> astersql_lightning_log::Logger + Send + Sync>,
    pub LocalEngines: Option<(
        Arc<astersql_lightning_backend::OpenedEngine>,
        Arc<astersql_lightning_backend::OpenedEngine>,
    )>,
    pub Collector: Option<Arc<dyn Collector + Send + Sync>>,
    /// Optional executor factory used by hosts that supply their own chunk implementation.
    pub WorkerFactory: Option<EncodeSortWorkerFactory>,
}

impl ConfiguredEncodeSortRuntime {
    pub fn importer_service(&self) -> Arc<dyn importer::TableImporterService> {
        self.SharedImporterService
            .get_or_init(|| (self.ImporterService)())
            .clone()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn NewConfiguredEncodeSortFactory(
    runtime: Arc<ConfiguredEncodeSortRuntime>,
    pool_context: PoolContext,
    meta: crate::proto::TaskMeta,
    task_id: i64,
    subtask_id: i64,
    data_memory: u64,
    index_memory: u64,
    data_block: usize,
    index_block: usize,
    summaries: Arc<GlobalWriterSummaries>,
) -> EncodeSortWorkerFactory {
    let meta = Arc::new(Mutex::new(meta));
    Arc::new(move || {
        let worker_context = Context::with_cancel_check(Arc::new({
            let pool_context = pool_context.clone();
            move || pool_context.IsCancelled()
        }));
        let meta = meta
            .lock()
            .map_err(|_| "encode sort task meta mutex poisoned".to_owned())?
            .clone();
        let table_info = meta
            .Plan
            .TableInfo
            .as_ref()
            .ok_or_else(|| "import task plan has no table info".to_owned())?;
        let local_engines = if meta.Plan.IsLocalSort() {
            Some(
                runtime
                    .LocalEngines
                    .clone()
                    .ok_or_else(|| "local engines are not initialized".to_owned())?,
            )
        } else {
            None
        };
        let table = astersql_table::BuildTableFromMeta(table_info)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "table metadata factory is not installed".to_owned())?;
        let args = importer::ASTArgsFromStmt(&meta.Stmt)?;
        let mut controller = importer::NewLoadDataController(
            meta.Plan.clone(),
            Arc::from(table),
            args,
            (runtime.ControllerServices)(),
            Vec::new(),
        )?;
        controller.InitDataStore(&astersql_objstore_storeapi::Context::default())?;
        let worker_id = uuid::Uuid::new_v4().to_string();
        let table_importer = importer::NewTableImporter(
            controller,
            format!("{task_id}-{subtask_id}-{worker_id}"),
            runtime.importer_service(),
        )?;
        let keyspace = table_importer.GetKeySpace();
        let indices = meta
            .Plan
            .DesiredTableInfo
            .as_deref()
            .or(meta.Plan.TableInfo.as_deref())
            .map(importer::GetIndicesGenKV)
            .unwrap_or_default();
        let worker = if local_engines.is_some() {
            chunkWorker {
                ctx: worker_context.clone(),
                dataWriter: None,
                indexWriter: None,
                collector: runtime.Collector.clone(),
            }
        } else {
            BuildGlobalChunkWorker(
                worker_context,
                task_id,
                subtask_id,
                &worker_id,
                runtime.ObjectStore.clone(),
                keyspace.clone(),
                indices,
                meta.Plan.GetOnDupKeyMode(),
                data_memory,
                index_memory,
                data_block,
                index_block,
                runtime.Collector.clone(),
                summaries.clone(),
            )
        };
        let shared = SharedVars {
            TableImporter: table_importer,
            DataEngine: local_engines.as_ref().map(|engines| engines.0.clone()),
            IndexEngine: local_engines.as_ref().map(|engines| engines.1.clone()),
            mu: Mutex::new(()),
            Checksum: astersql_lightning_verification::NewKVGroupChecksumWithKeyspace(&keyspace),
            SortedDataMeta: SortedKVMeta::default(),
            SortedIndexMetas: HashMap::new(),
            RecordedConflictKVCount: 0,
            ShareMu: Mutex::new(()),
            globalSortStore: None,
            dataKVFileCount: AtomicI64::new(0),
            indexKVFileCount: AtomicI64::new(0),
        };
        Ok(Box::new(ThreadOwnedEncodeSortWorker::new(
            worker,
            shared,
            (runtime.LoggerFactory)(),
            summaries.clone(),
        )) as Box<dyn EncodeSortWorker>)
    })
}

impl ThreadOwnedEncodeSortWorker {
    pub fn new(
        worker: chunkWorker,
        shared: SharedVars,
        logger: astersql_lightning_log::Logger,
        summaries: Arc<GlobalWriterSummaries>,
    ) -> Self {
        Self {
            worker,
            shared: Some(shared),
            logger,
            summaries,
        }
    }
}

impl EncodeSortWorker for ThreadOwnedEncodeSortWorker {
    fn HandleTask(&mut self, task: EncodeSortTask) -> Result<(), String> {
        let shared = self
            .shared
            .take()
            .ok_or_else(|| "encode sort worker state is missing".to_owned())?;
        let mut minimal = importStepMinimalTask {
            Plan: task.Plan,
            Chunk: task.Chunk,
            SharedVars: shared,
            logger: self.logger.clone(),
        };
        let result = self
            .worker
            .HandleTask(&mut minimal)
            .map_err(|error| error.to_string());
        self.shared = Some(minimal.SharedVars);
        result
    }

    fn Close(&mut self) -> Result<(), String> {
        let writer_error = self.worker.Close().err().map(|error| error.to_string());
        if let Some(shared) = self.shared.as_mut() {
            self.summaries.merge_checksum(&shared.Checksum);
            self.summaries
                .merge_allocator_maximums(&shared.TableImporter.AllocatorMaximums());
            shared.TableImporter.Close();
        }
        writer_error.map_or(Ok(()), Err)
    }
}

/// Async operator with Go's first-error cancellation and parallel worker drain.
pub struct AsyncEncodeSortOperator {
    pub taskID: i64,
    pub subtaskID: i64,
    pub taskKeyspace: String,
    context: PoolContext,
    concurrency: usize,
    factory: EncodeSortWorkerFactory,
    source: Option<SimpleDataChannel<EncodeSortTask>>,
    workers: Vec<JoinHandle<()>>,
}

pub fn newEncodeAndSortOperator(
    context: PoolContext,
    task_id: i64,
    subtask_id: i64,
    keyspace: impl Into<String>,
    concurrency: usize,
    factory: EncodeSortWorkerFactory,
) -> AsyncEncodeSortOperator {
    AsyncEncodeSortOperator {
        taskID: task_id,
        subtaskID: subtask_id,
        taskKeyspace: keyspace.into(),
        context,
        concurrency: concurrency.max(1),
        factory,
        source: None,
        workers: Vec::new(),
    }
}

impl WithSource<EncodeSortTask> for AsyncEncodeSortOperator {
    fn SetSource(&mut self, source: SimpleDataChannel<EncodeSortTask>) {
        self.source = Some(source);
    }
}

impl Operator for AsyncEncodeSortOperator {
    fn Open(&mut self) -> Result<(), PoolError> {
        let source = self
            .source
            .clone()
            .ok_or_else(|| PoolError::new("encode sort source is not set"))?;
        let (ready_sender, ready_receiver) = mpsc::channel();
        for _ in 0..self.concurrency {
            let factory = self.factory.clone();
            let input = source.clone();
            let context = self.context.clone();
            let ready = ready_sender.clone();
            self.workers.push(thread::spawn(move || {
                let mut worker = match factory() {
                    Ok(worker) => {
                        let _ = ready.send(Ok(()));
                        worker
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error.clone()));
                        context.OnError(PoolError::new(error));
                        input.Finish();
                        return;
                    }
                };
                while !context.IsCancelled() {
                    let Some(task) = input.Channel().recv() else {
                        break;
                    };
                    if let Err(error) = worker.HandleTask(task) {
                        context.OnError(PoolError::new(error));
                        input.Finish();
                        break;
                    }
                }
                if let Err(error) = worker.Close() {
                    context.OnError(PoolError::new(error));
                }
            }));
        }
        drop(ready_sender);
        let mut first_error = None;
        for _ in 0..self.concurrency {
            match ready_receiver.recv() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    first_error.get_or_insert(error);
                }
                Err(_) => {
                    first_error.get_or_insert(
                        "encode sort worker exited during initialization".to_owned(),
                    );
                }
            }
        }
        if let Some(error) = first_error {
            source.Finish();
            return Err(PoolError::new(error));
        }
        Ok(())
    }

    fn Close(&mut self) -> Result<(), PoolError> {
        if let Some(source) = &self.source {
            source.Finish();
        }
        for handle in self.workers.drain(..) {
            if handle.join().is_err() {
                self.context
                    .OnError(PoolError::new("encode sort worker panicked"));
            }
        }
        Ok(())
    }

    fn String(&self) -> String {
        "encodeAndSortOperator".into()
    }
}

/// Drive the encode-and-sort workers for one already decoded import subtask.
/// The owning step executor prepares local engines and handles their import,
/// allocator bases and external subtask metadata after this call returns.
#[allow(clippy::too_many_arguments)]
pub fn RunEncodeSortChunks(
    context: PoolContext,
    task_meta: crate::proto::TaskMeta,
    step_meta: &mut crate::proto::ImportStepMeta,
    runtime: Arc<ConfiguredEncodeSortRuntime>,
    task_id: i64,
    subtask_id: i64,
    concurrency: usize,
    data_memory: u64,
    index_memory: u64,
    data_block: usize,
    index_block: usize,
) -> Result<(), errors::SharedError> {
    let summaries = Arc::new(GlobalWriterSummaries::default());
    let factory = runtime.WorkerFactory.clone().unwrap_or_else(|| {
        NewConfiguredEncodeSortFactory(
            runtime,
            context.clone(),
            task_meta.clone(),
            task_id,
            subtask_id,
            data_memory,
            index_memory,
            data_block,
            index_block,
            summaries.clone(),
        )
    });
    let mut operator = newEncodeAndSortOperator(
        context.clone(),
        task_id,
        subtask_id,
        task_meta.Plan.Keyspace.clone(),
        concurrency,
        factory,
    );
    let source = NewSimpleDataChannel(Channel::bounded(0));
    operator.SetSource(source.clone());
    if let Err(error) = operator.Open() {
        let _ = operator.Close();
        return Err(errors::New(error.to_string()));
    }
    let mut send_error = None;
    for chunk in &step_meta.Chunks {
        if !source.Channel().send(EncodeSortTask {
            Plan: task_meta.Plan.clone(),
            Chunk: chunk.clone(),
        }) {
            send_error = Some(errors::New(
                "encode sort source closed before all chunks were sent",
            ));
            break;
        }
    }
    source.Finish();
    operator
        .Close()
        .map_err(|error| errors::New(error.to_string()))?;
    if let Some(error) = context.OperatorErr() {
        return Err(errors::New(error.to_string()));
    }
    if let Some(error) = send_error {
        return Err(error);
    }

    step_meta.Checksum = summaries
        .checksum()
        .GetInnerChecksums()
        .into_iter()
        .map(|(id, checksum)| (id, crate::proto::newFromKVChecksum(&checksum)))
        .collect();
    for (kind, maximum) in summaries.allocator_maximums() {
        let kind = match kind {
            astersql_lightning_backend_kv::AllocatorType::RowIDAllocType => {
                astersql_meta_autoid::AllocatorType::RowId
            }
            astersql_lightning_backend_kv::AllocatorType::AutoIncrementType => {
                astersql_meta_autoid::AllocatorType::AutoIncrement
            }
            astersql_lightning_backend_kv::AllocatorType::AutoRandomType => {
                astersql_meta_autoid::AllocatorType::AutoRandom
            }
        };
        step_meta
            .MaxIDs
            .entry(kind)
            .and_modify(|current| *current = (*current).max(maximum))
            .or_insert(maximum);
    }
    let data_meta = summaries.data_meta();
    let index_metas = summaries.index_metas();
    step_meta.RecordedConflictKVCount = index_metas
        .values()
        .fold(data_meta.ConflictInfo.Count, |total, meta| {
            total.wrapping_add(meta.ConflictInfo.Count)
        });
    step_meta.SortedDataMeta = Some(data_meta);
    step_meta.SortedIndexMetas = index_metas;
    Ok(())
}
use crate::subtask_executor::runImportMinimalTask;
use crate::task_executor::writerMemBudgetRatio;

/// Bridge the SST writer to the task's real object store. The caller closes
/// the store after every worker has flushed and reported its summaries.
pub struct ObjectStoreWriterSink {
    pub store: StorageRef,
    context: Mutex<Context>,
}

impl ObjectStoreWriterSink {
    pub fn new(store: StorageRef, context: Context) -> Self {
        Self {
            store,
            context: Mutex::new(context),
        }
    }

    pub fn set_context(&self, context: Context) {
        *self.context.lock().unwrap() = context;
    }
}

impl WriterSink for ObjectStoreWriterSink {
    fn write_file(&self, path: &str, data: &[u8]) -> Result<(), String> {
        let encode_context = self
            .context
            .lock()
            .map_err(|_| "object sink context mutex poisoned".to_owned())?
            .clone();
        if encode_context.is_cancelled() {
            return Err("object write was cancelled".into());
        }
        let store_context = StoreContext::default();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = {
            let store_context = store_context.clone();
            let done = done.clone();
            thread::spawn(move || {
                while !done.load(Ordering::Acquire) {
                    if encode_context.is_cancelled() {
                        store_context.cancel();
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            })
        };
        let result = self
            .store
            .WriteFile(&store_context, path, data)
            .map_err(|error| error.to_string());
        done.store(true, Ordering::Release);
        let _ = watcher.join();
        result
    }
}

/// Adapt the global-sort SST writer to the real importer EngineWriter contract.
pub struct GlobalDataEngineWriter {
    writer: Writer,
    summary: Option<WriterSummary>,
    sink: Option<Arc<ObjectStoreWriterSink>>,
}

impl GlobalDataEngineWriter {
    pub fn new(writer: Writer) -> Self {
        Self {
            writer,
            summary: None,
            sink: None,
        }
    }

    pub fn with_sink(writer: Writer, sink: Arc<ObjectStoreWriterSink>) -> Self {
        Self {
            writer,
            summary: None,
            sink: Some(sink),
        }
    }

    pub fn summary(&self) -> Option<&WriterSummary> {
        self.summary.as_ref()
    }
}

impl EngineWriter for GlobalDataEngineWriter {
    fn AppendRows(
        &mut self,
        _: &Context,
        _: &[String],
        rows: &dyn Rows,
    ) -> Result<(), BackendError> {
        for pair in Rows2KvPairs(rows) {
            self.writer
                .write_row(&pair.key, &pair.val)
                .map_err(|error| BackendError::new(error.to_string()))?;
        }
        Ok(())
    }

    fn IsSynced(&self) -> bool {
        true
    }

    fn Close(&mut self, context: &Context) -> Result<Option<ChunkFlushStatus>, BackendError> {
        if let Some(sink) = &self.sink {
            sink.set_context(context.clone());
        }
        let summary = self
            .writer
            .close()
            .map_err(|error| BackendError::new(error.to_string()))?;
        self.summary = Some(summary);
        Ok(Some(ChunkFlushStatus { flushed: true }))
    }
}

/// The per-index SST writer used by importer's lazy IndexRouteWriter.
pub struct GlobalIndexSstWriter {
    writer: Writer,
    sink: Option<Arc<ObjectStoreWriterSink>>,
}

impl GlobalIndexSstWriter {
    pub fn new(writer: Writer) -> Self {
        Self { writer, sink: None }
    }

    pub fn with_sink(writer: Writer, sink: Arc<ObjectStoreWriterSink>) -> Self {
        Self {
            writer,
            sink: Some(sink),
        }
    }
}

impl RoutedIndexWriter for GlobalIndexSstWriter {
    fn WriteRow(&mut self, _: &Context, key: &[u8], value: &[u8]) -> Result<(), String> {
        self.writer
            .write_row(key, value)
            .map_err(|error| error.to_string())
    }

    fn Close(&mut self, context: &Context) -> Result<(), String> {
        if let Some(sink) = &self.sink {
            sink.set_context(context.clone());
        }
        self.writer
            .close()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Thread-safe wire summary; non-Send importer state remains inside each worker.
pub struct GlobalWriterSummaries {
    data: Mutex<SortedKVMeta>,
    indices: Mutex<HashMap<i64, SortedKVMeta>>,
    data_files: AtomicI64,
    index_files: AtomicI64,
    checksum: Mutex<astersql_lightning_verification::KVGroupChecksum>,
    maximum_ids: Mutex<HashMap<astersql_lightning_backend_kv::AllocatorType, i64>>,
}

impl Default for GlobalWriterSummaries {
    fn default() -> Self {
        Self {
            data: Mutex::new(SortedKVMeta::default()),
            indices: Mutex::new(HashMap::new()),
            data_files: AtomicI64::new(0),
            index_files: AtomicI64::new(0),
            checksum: Mutex::new(astersql_lightning_verification::NewKVGroupChecksumForAdd()),
            maximum_ids: Mutex::new(HashMap::new()),
        }
    }
}

impl GlobalWriterSummaries {
    pub fn merge_checksum(&self, checksum: &astersql_lightning_verification::KVGroupChecksum) {
        self.checksum.lock().unwrap().Add(checksum);
    }
    pub fn checksum(&self) -> astersql_lightning_verification::KVGroupChecksum {
        self.checksum.lock().unwrap().clone()
    }
    pub fn merge_allocator_maximums(
        &self,
        maximums: &HashMap<astersql_lightning_backend_kv::AllocatorType, i64>,
    ) {
        let mut merged = self.maximum_ids.lock().unwrap();
        for (kind, maximum) in maximums {
            merged
                .entry(*kind)
                .and_modify(|current| *current = (*current).max(*maximum))
                .or_insert(*maximum);
        }
    }
    pub fn allocator_maximums(&self) -> HashMap<astersql_lightning_backend_kv::AllocatorType, i64> {
        self.maximum_ids.lock().unwrap().clone()
    }
    fn merge_data(&self, summary: &WriterSummary) {
        self.data
            .lock()
            .unwrap()
            .MergeSummary(&protocol_writer_summary(summary));
        self.data_files
            .fetch_add(summary.KVFileCount as i64, Ordering::Relaxed);
    }
    fn merge_index(&self, index_id: i64, summary: &WriterSummary) {
        self.indices
            .lock()
            .unwrap()
            .entry(index_id)
            .or_default()
            .MergeSummary(&protocol_writer_summary(summary));
        self.index_files
            .fetch_add(summary.KVFileCount as i64, Ordering::Relaxed);
    }
    pub fn data_meta(&self) -> SortedKVMeta {
        self.data.lock().unwrap().clone()
    }
    pub fn index_meta(&self, index_id: i64) -> Option<SortedKVMeta> {
        self.indices.lock().unwrap().get(&index_id).cloned()
    }
    pub fn index_metas(&self) -> HashMap<i64, SortedKVMeta> {
        self.indices.lock().unwrap().clone()
    }
    pub fn data_file_count(&self) -> i64 {
        self.data_files.load(Ordering::Relaxed)
    }
    pub fn index_file_count(&self) -> i64 {
        self.index_files.load(Ordering::Relaxed)
    }
}

fn duplicate_mode(mode: astersql_ingestor_engineapi::OnDuplicateKey) -> DuplicateMode {
    match mode.0 {
        1 => DuplicateMode::Record,
        2 => DuplicateMode::Remove,
        3 => DuplicateMode::Error,
        _ => DuplicateMode::Ignore,
    }
}

/// Create the real data writer and lazy per-index route writer for one worker.
#[allow(clippy::too_many_arguments)]
pub fn BuildGlobalChunkWorker(
    ctx: Context,
    task_id: i64,
    subtask_id: i64,
    worker_id: &str,
    store: StorageRef,
    key_prefix: Vec<u8>,
    indices: HashMap<i64, importer::GenKVIndex>,
    on_dup: importer::OnDupKeyMode,
    data_memory: u64,
    index_memory: u64,
    data_block: usize,
    index_block: usize,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
    summaries: Arc<GlobalWriterSummaries>,
) -> chunkWorker {
    let prefix = subtaskPrefix(task_id, subtask_id);
    let sink = Arc::new(ObjectStoreWriterSink::new(store, ctx.clone()));
    let mut data_builder = WriterBuilder::new();
    let data_summaries = summaries.clone();
    data_builder
        .set_key_prefix(key_prefix.clone())
        .set_memory_size_limit(data_memory)
        .set_block_size(data_block)
        .set_on_duplicate(duplicate_mode(
            crate::task_executor::getOnDupForConflictedKV(on_dup),
        ))
        .set_on_close(move |summary| data_summaries.merge_data(summary));
    let data_id = format!("data/{worker_id}");
    let data_writer = GlobalDataEngineWriter::with_sink(
        data_builder.build_with_sink(sink.clone(), &prefix, &data_id),
        sink.clone(),
    );

    let worker_id = worker_id.to_owned();
    let factory: importer::WriterFactory = Arc::new(move |index_id| {
        let on_dup = crate::task_executor::getOnDupForIndex(&indices, index_id, on_dup)
            .map_err(|error| error.to_string())?;
        let index_summaries = summaries.clone();
        let mut builder = WriterBuilder::new();
        builder
            .set_key_prefix(key_prefix.clone())
            .set_memory_size_limit(index_memory)
            .set_block_size(index_block)
            .set_on_duplicate(duplicate_mode(on_dup))
            .set_on_close(move |summary| index_summaries.merge_index(index_id, summary));
        let writer_id = format!("index/{index_id}/{worker_id}");
        Ok(Box::new(GlobalIndexSstWriter::with_sink(
            builder.build_with_sink(sink.clone(), &prefix, &writer_id),
            sink.clone(),
        )))
    });
    let index_writer = importer::NewIndexRouteWriter(Default::default(), factory);
    chunkWorker {
        ctx,
        dataWriter: Some(Arc::new(Mutex::new(Box::new(data_writer)))),
        indexWriter: Some(Arc::new(Mutex::new(Box::new(index_writer)))),
        collector,
    }
}

/// worker/算子等待资源或完成的最长时长。
pub const maxWaitDuration: Duration = Duration::from_secs(30);

/// 单个 encode-and-sort 子任务内各 worker 共享的运行时状态。
/// Runtime state shared by the workers of one encode-and-sort subtask.
pub struct encodeAndSortOperator {
    /// 所属分布式任务 ID。
    pub taskID: i64,
    /// 当前子任务 ID。
    pub subtaskID: i64,
    /// 任务所在 keyspace（多租户隔离名）。
    pub taskKeyspace: String,
    /// 跨 worker 共享的导入变量（计划、导入器状态等）。
    pub sharedVars: SharedVars,
    /// 进度采集器；None 表示不汇报。
    pub collector: Option<Arc<dyn Collector + Send + Sync>>,
}

impl encodeAndSortOperator {
    /// 算子名称，用于日志与调试标识。
    pub fn String(&self) -> &'static str {
        "encodeAndSortOperator"
    }
}

/// 单个编码 worker。`dataWriter`/`indexWriter` 为 Lightning `EngineWriter`；
/// 本地排序场景刻意置空，由 importer 自行打开 local-engine writer。
/// One encode worker. Writers are the real Lightning `EngineWriter` trait;
/// local-sort workers intentionally keep them empty because the importer opens
/// local-engine writers itself.
pub struct chunkWorker {
    /// 编码上下文（会话/取消等）。
    pub ctx: Context,
    /// 数据 KV 写入器；本地排序时常为 None。
    pub dataWriter: Option<Arc<Mutex<Box<dyn EngineWriter>>>>,
    /// 索引 KV 写入器；本地排序时常为 None。
    pub indexWriter: Option<Arc<Mutex<Box<dyn EngineWriter>>>>,
    /// 本 worker 的进度采集器。
    pub collector: Option<Arc<dyn Collector + Send + Sync>>,
}

/// importer 每个 chunk 接收一个 Box writer，并在处理结束时调用 Close。
/// 共享代理将 Close 留给 worker 的生命周期，避免首个 chunk 消费底层 writer。
struct ChunkWriterProxy {
    inner: Arc<Mutex<Box<dyn EngineWriter>>>,
}

impl EngineWriter for ChunkWriterProxy {
    fn AppendRows(
        &mut self,
        ctx: &Context,
        names: &[String],
        rows: &dyn Rows,
    ) -> Result<(), BackendError> {
        self.inner
            .lock()
            .map_err(|_| BackendError::new("chunk writer mutex poisoned"))?
            .AppendRows(ctx, names, rows)
    }

    fn IsSynced(&self) -> bool {
        self.inner
            .lock()
            .map(|writer| writer.IsSynced())
            .unwrap_or(false)
    }

    fn Close(&mut self, _: &Context) -> Result<Option<ChunkFlushStatus>, BackendError> {
        Ok(None)
    }
}

impl chunkWorker {
    pub(crate) fn chunkWriters(
        &self,
    ) -> (Option<Box<dyn EngineWriter>>, Option<Box<dyn EngineWriter>>) {
        let proxy = |inner: &Arc<Mutex<Box<dyn EngineWriter>>>| {
            Box::new(ChunkWriterProxy {
                inner: inner.clone(),
            }) as Box<dyn EngineWriter>
        };
        (
            self.dataWriter.as_ref().map(proxy),
            self.indexWriter.as_ref().map(proxy),
        )
    }

    /// 处理一个 minimal task：编码并写入 data/index writer。
    pub fn HandleTask(
        &mut self,
        task: &mut importStepMinimalTask,
    ) -> Result<(), errors::SharedError> {
        let (data_writer, index_writer) = self.chunkWriters();
        runImportMinimalTask(
            &self.ctx,
            task,
            data_writer,
            index_writer,
            self.collector.clone(),
        )
    }

    /// 取消后使用可继续写入的关闭上下文；data 关闭失败立即返回。
    pub fn Close(&mut self) -> Result<(), errors::SharedError> {
        let close_ctx = if self.ctx.is_cancelled() {
            Context::with_timeout(maxWaitDuration)
        } else {
            self.ctx.clone()
        };
        if let Some(writer) = self.dataWriter.as_mut() {
            writer
                .lock()
                .map_err(|_| errors::New("data writer mutex poisoned"))?
                .Close(&close_ctx)
                .map_err(|error| errors::New(error.to_string()))?;
        }
        if let Some(writer) = self.indexWriter.as_mut() {
            writer
                .lock()
                .map_err(|_| errors::New("index writer mutex poisoned"))?
                .Close(&close_ctx)
                .map_err(|error| errors::New(error.to_string()))?;
        }
        Ok(())
    }
}

/// 返回对象存储前缀 `{task-id}/{subtask-id}`。
/// Returns the object-store prefix `{task-id}/{subtask-id}`.
pub fn subtaskPrefix(taskID: i64, subtaskID: i64) -> String {
    Path::new(&taskID.to_string())
        .join(subtaskID.to_string())
        .to_string_lossy()
        .into_owned()
}

/// 将 worker 可用内存的一半按 `index-group-count + 3` 份分摊：
/// 数据 writer 占 3 份，每个索引 writer 占 1 份，与 Go 实现一致。
/// Divide half of the memory available to a worker into
/// `index-group-count + 3` shares. The data writer owns three shares and every
/// index writer owns one, exactly matching the Go implementation.
pub fn getWriterMemorySizeLimit(resource: &StepResource, plan: &importer::Plan) -> (u64, u64) {
    // 索引组数决定分母；无 DesiredTableInfo 时视为 0。
    let index_group_count = plan
        .DesiredTableInfo
        .as_deref()
        .map(importer::GetNumOfIndexGenKV)
        .unwrap_or(0);
    // MemoryPerCore 为每核内存；乘 writerMemBudgetRatio 后再按份额切分。
    let memory_per_worker = resource.MemoryPerCore().max(0) as f64;
    let memory_per_share =
        memory_per_worker * writerMemBudgetRatio / (index_group_count.saturating_add(3) as f64);
    ((memory_per_share * 3.0) as u64, memory_per_share as u64)
}
