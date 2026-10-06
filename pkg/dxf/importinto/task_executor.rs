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

// Import Into 编码/导入步骤执行器：内存预算、并发估计与重复键策略。
//
// `importStepExecutor` 在 TableImporter 构建完成后持有每连接的 data/index KV
// writer 内存上限、块大小与并发度，并作为 Collector 汇总子任务进度。
// 另提供按 KV 组（数据行或索引）映射 OnDuplicateKey（重复键处理）策略的辅助函数，
// 以及 ingest 阶段的计量收集器。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use astersql_dxf_framework_metering as metering;
use astersql_dxf_framework_proto::subtask::StepResource;
use astersql_dxf_framework_proto::{step, task::Task};
use astersql_dxf_framework_taskexecutor as node_executor;
use astersql_dxf_framework_taskexecutor_execute as execute;
use astersql_errors as errors;
use astersql_executor_importer as importer;
use astersql_ingestor_engineapi as engineapi;
use astersql_ingestor_globalsort as globalsort;
use astersql_lightning_backend_encode::Context as EncodeContext;
use astersql_lightning_common as lightning_common;
use astersql_lightning_mydump::SourceType;
use astersql_objstore_storeapi::{Context as ObjectContext, StorageRef};
use astersql_resourcemanager_pool_workerpool::Context as PoolContext;

use crate::encode_and_sort_operator::{
    ConfiguredEncodeSortRuntime, RunEncodeSortChunks, getWriterMemorySizeLimit,
};

use crate::proto::*;

struct SubtaskRequestRecorder<'a> {
    store: StorageRef,
    before: Option<(u64, u64)>,
    summary: &'a execute::SubtaskSummary,
}
impl<'a> SubtaskRequestRecorder<'a> {
    fn new(store: StorageRef, summary: &'a execute::SubtaskSummary) -> Self {
        Self {
            before: store.AccessRequestSnapshot(),
            store,
            summary,
        }
    }
}
impl Drop for SubtaskRequestRecorder<'_> {
    fn drop(&mut self) {
        if let (Some((before_get, before_put)), Some((get, put))) =
            (self.before, self.store.AccessRequestSnapshot())
        {
            self.summary
                .GetReqCnt
                .fetch_add(get.saturating_sub(before_get), Ordering::Relaxed);
            self.summary
                .PutReqCnt
                .fetch_add(put.saturating_sub(before_put), Ordering::Relaxed);
        }
    }
}

/// Encode-and-sort step called by the distributed task framework.
pub struct EncodeSortStepExecutor {
    task_id: i64,
    task_meta: TaskMeta,
    runtime: Arc<ConfiguredEncodeSortRuntime>,
    summary: execute::SubtaskSummary,
    framework: Option<execute::FrameworkInfo>,
    parent_importer: Option<Box<dyn EncodeSortImporterHost>>,
    parquet_estimated: bool,
    concurrency: usize,
}

/// The Go failpoint tests isolate table-importer construction and the local
/// engine backend. Keep the same boundary injectable while production uses
/// the complete TableImporter implementation.
pub trait EncodeSortImporterHost {
    fn EstimateParquetReaderMemory(&self, path: &str, file_size: i64) -> Result<i64, String>;
    fn OpenDataEngine(
        &self,
        context: &EncodeContext,
        engine_id: i32,
    ) -> Result<astersql_lightning_backend::OpenedEngine, String>;
    fn OpenIndexEngine(
        &self,
        context: &EncodeContext,
        engine_id: i32,
    ) -> Result<astersql_lightning_backend::OpenedEngine, String>;
    fn ImportAndCleanup(
        &self,
        context: &EncodeContext,
        closed: &astersql_lightning_backend::ClosedEngine,
    ) -> Result<i64, String>;
    fn CleanupAllLocalEngines(&self, context: &EncodeContext);
    fn Close(&mut self);
}

impl EncodeSortImporterHost for importer::TableImporter {
    fn EstimateParquetReaderMemory(&self, path: &str, file_size: i64) -> Result<i64, String> {
        self.EstimateParquetReaderMemory(path, file_size)
    }
    fn OpenDataEngine(
        &self,
        context: &EncodeContext,
        engine_id: i32,
    ) -> Result<astersql_lightning_backend::OpenedEngine, String> {
        self.OpenDataEngine(context, engine_id)
    }
    fn OpenIndexEngine(
        &self,
        context: &EncodeContext,
        engine_id: i32,
    ) -> Result<astersql_lightning_backend::OpenedEngine, String> {
        self.OpenIndexEngine(context, engine_id)
    }
    fn ImportAndCleanup(
        &self,
        context: &EncodeContext,
        closed: &astersql_lightning_backend::ClosedEngine,
    ) -> Result<i64, String> {
        self.ImportAndCleanup(context, closed)
    }
    fn CleanupAllLocalEngines(&self, context: &EncodeContext) {
        self.CleanupAllLocalEngines(context)
    }
    fn Close(&mut self) {
        self.Close()
    }
}

pub fn NewEncodeSortStepExecutor(
    task_id: i64,
    task_meta: TaskMeta,
    runtime: Arc<ConfiguredEncodeSortRuntime>,
) -> EncodeSortStepExecutor {
    EncodeSortStepExecutor {
        task_id,
        task_meta,
        runtime,
        summary: execute::SubtaskSummary::default(),
        framework: None,
        parent_importer: None,
        parquet_estimated: false,
        concurrency: 0,
    }
}

pub fn GetEncodeSortStepExecutor(
    task: &Task,
    runtime: Arc<ConfiguredEncodeSortRuntime>,
) -> Result<Box<dyn execute::StepExecutor>, errors::SharedError> {
    let meta = TaskMeta::Unmarshal(&task.Meta)?;
    if task.Step != step::ImportStepEncodeAndSort {
        return Err(errors::New(format!(
            "unknown step {} for import task {}",
            task.Step, task.ID
        )));
    }
    Ok(Box::new(NewEncodeSortStepExecutor(task.ID, meta, runtime)))
}

impl execute::StepExecFrameworkInfo for EncodeSortStepExecutor {
    fn restricted(&self) {}
    fn GetStep(&self) -> step::Step {
        self.framework
            .as_ref()
            .map_or(0, execute::StepExecFrameworkInfo::GetStep)
    }
    fn GetResource(&self) -> Option<Arc<StepResource>> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetResource)
    }
    fn SetResource(&self, resource: Arc<StepResource>) {
        if let Some(framework) = &self.framework {
            framework.SetResource(resource);
        }
    }
    fn GetMeterRecorder(&self) -> Option<Arc<metering::Recorder>> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetMeterRecorder)
    }
    fn GetCheckpointUpdateFunc(&self) -> Option<execute::CheckpointUpdateFunc> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetCheckpointUpdateFunc)
    }
    fn GetCheckpointFunc(&self) -> Option<execute::CheckpointGetFunc> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetCheckpointFunc)
    }
}

impl EncodeSortStepExecutor {
    pub fn SetImporterHost(&mut self, host: Box<dyn EncodeSortImporterHost>) {
        self.parent_importer = Some(host);
    }

    fn read_meta(
        &self,
        bytes: &[u8],
        store: &StorageRef,
    ) -> Result<ImportStepMeta, errors::SharedError> {
        let mut meta = ImportStepMeta::Unmarshal(bytes)?;
        if !meta.BaseExternalMeta.ExternalPath.is_empty() {
            let data = store
                .ReadFile(
                    &ObjectContext::default(),
                    &meta.BaseExternalMeta.ExternalPath,
                )
                .map_err(|error| errors::New(error.to_string()))?;
            let mut external = ImportStepMeta::Unmarshal(&data)?;
            external.BaseExternalMeta.ExternalPath = meta.BaseExternalMeta.ExternalPath;
            meta = external;
        }
        Ok(meta)
    }

    fn build_parent_importer(
        &self,
        subtask_id: i64,
    ) -> Result<Box<dyn EncodeSortImporterHost>, errors::SharedError> {
        let table_info = self
            .task_meta
            .Plan
            .TableInfo
            .as_ref()
            .ok_or_else(|| errors::New("import task plan has no table info"))?;
        let table = astersql_table::BuildTableFromMeta(table_info)
            .map_err(|error| errors::New(error.to_string()))?
            .ok_or_else(|| errors::New("table metadata factory is not installed"))?;
        let args = importer::ASTArgsFromStmt(&self.task_meta.Stmt).map_err(errors::New)?;
        let mut controller = importer::NewLoadDataController(
            self.task_meta.Plan.clone(),
            Arc::from(table),
            args,
            (self.runtime.ControllerServices)(),
            Vec::new(),
        )
        .map_err(errors::New)?;
        controller
            .InitDataStore(&astersql_objstore_storeapi::Context::default())
            .map_err(errors::New)?;
        importer::NewTableImporter(
            controller,
            format!("{}-{subtask_id}-parent", self.task_id),
            self.runtime.importer_service(),
        )
        .map(|importer| Box::new(importer) as Box<dyn EncodeSortImporterHost>)
        .map_err(errors::New)
    }

    fn run(
        &mut self,
        context: execute::Context,
        subtask: &mut astersql_dxf_framework_proto::subtask::Subtask,
    ) -> anyhow::Result<()> {
        if context.is_cancelled() {
            return Err(anyhow::anyhow!("encode sort cancelled"));
        }
        let object_store = if let Some(factory) = &self.runtime.ObjectStoreFactory {
            factory().map_err(|error| anyhow::anyhow!(error))?
        } else {
            self.runtime.ObjectStore.clone()
        };
        struct StoreGuard {
            store: StorageRef,
            owned: bool,
        }
        impl Drop for StoreGuard {
            fn drop(&mut self) {
                if self.owned {
                    self.store.Close();
                }
            }
        }
        let _store_guard = StoreGuard {
            store: object_store.clone(),
            owned: self.runtime.ObjectStoreFactory.is_some(),
        };
        let _requests = self
            .runtime
            .ObjectStoreFactory
            .as_ref()
            .map(|_| SubtaskRequestRecorder::new(object_store.clone(), &self.summary));
        let mut step_meta = self.read_meta(&subtask.Meta, &object_store)?;
        (self.runtime.LoggerFactory)()
            .With([astersql_lightning_log::Field::int("subtask-id", subtask.ID)])
            .Info(
                "start processing chunks",
                [astersql_lightning_log::Field::int(
                    "chunkCount",
                    step_meta.Chunks.len() as i64,
                )],
            );
        let resource = execute::StepExecFrameworkInfo::GetResource(self)
            .ok_or_else(|| anyhow::anyhow!("encode sort resource is unavailable"))?;
        let (data_memory, index_memory) = getWriterMemorySizeLimit(&resource, &self.task_meta.Plan);
        let data_block = getAdjustedBlockSize(data_memory, maxTxnEntrySizeLimit);
        let index_block = getAdjustedBlockSize(index_memory, defaultBlockSize);
        if !self.parquet_estimated {
            self.parquet_estimated = true;
            self.concurrency = resource.CPU.Capacity().max(1) as usize;
            if let Some(parent) = self.parent_importer.as_ref() {
                self.concurrency = parquet_reader_concurrency(
                    &step_meta.Chunks,
                    self.concurrency,
                    resource.Mem.Capacity(),
                    |path, size| parent.EstimateParquetReaderMemory(path, size),
                );
            }
        }
        let concurrency = self.concurrency;
        let pool = PoolContext::background();
        let encode_context = EncodeContext::with_cancel_check(Arc::new({
            let pool = pool.clone();
            move || pool.IsCancelled()
        }));
        let local_runtime = if self.task_meta.Plan.IsLocalSort() {
            let importer = self
                .parent_importer
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("encode sort importer is not initialized"))?;
            let data_engine = match importer.OpenDataEngine(&encode_context, step_meta.ID) {
                Ok(engine) => Arc::new(engine),
                Err(error) => {
                    importer.CleanupAllLocalEngines(&EncodeContext::default());
                    return Err(anyhow::anyhow!(error));
                }
            };
            let index_engine = match importer
                .OpenIndexEngine(&encode_context, importer::IndexEngineID - step_meta.ID)
            {
                Ok(engine) => Arc::new(engine),
                Err(error) => {
                    importer.CleanupAllLocalEngines(&EncodeContext::default());
                    return Err(anyhow::anyhow!(error));
                }
            };
            Arc::new(ConfiguredEncodeSortRuntime {
                ControllerServices: self.runtime.ControllerServices.clone(),
                ImporterService: self.runtime.ImporterService.clone(),
                SharedImporterService: self.runtime.SharedImporterService.clone(),
                ObjectStore: object_store.clone(),
                ObjectStoreFactory: None,
                LoggerFactory: self.runtime.LoggerFactory.clone(),
                LocalEngines: Some((data_engine, index_engine)),
                Collector: self.runtime.Collector.clone(),
                WorkerFactory: self.runtime.WorkerFactory.clone(),
            })
        } else {
            Arc::new(ConfiguredEncodeSortRuntime {
                ControllerServices: self.runtime.ControllerServices.clone(),
                ImporterService: self.runtime.ImporterService.clone(),
                SharedImporterService: self.runtime.SharedImporterService.clone(),
                ObjectStore: object_store.clone(),
                ObjectStoreFactory: None,
                LoggerFactory: self.runtime.LoggerFactory.clone(),
                LocalEngines: None,
                Collector: self.runtime.Collector.clone(),
                WorkerFactory: self.runtime.WorkerFactory.clone(),
            })
        };
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = {
            let context = context.clone();
            let pool = pool.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                while !done.load(Ordering::Acquire) {
                    if context.is_cancelled() {
                        pool.Cancel();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            })
        };
        let result = RunEncodeSortChunks(
            pool,
            self.task_meta.clone(),
            &mut step_meta,
            local_runtime.clone(),
            self.task_id,
            subtask.ID,
            concurrency,
            data_memory,
            index_memory,
            data_block,
            index_block,
        );
        done.store(true, Ordering::Release);
        let _ = watcher.join();
        if let Err(error) = result {
            if let Some(importer) = self
                .parent_importer
                .as_ref()
                .filter(|_| self.task_meta.Plan.IsLocalSort())
            {
                // Go cleans local engines after every failed import subtask so
                // opening the same engine IDs on retry cannot report already exists.
                importer.CleanupAllLocalEngines(&EncodeContext::default());
            }
            return Err(error.into());
        }
        if let Some(importer) = self
            .parent_importer
            .as_ref()
            .filter(|_| self.task_meta.Plan.IsLocalSort())
        {
            let import_result = (|| -> anyhow::Result<()> {
                let (data_engine, index_engine) =
                    local_runtime.LocalEngines.as_ref().unwrap().clone();
                drop(local_runtime);
                let closed_data = Arc::try_unwrap(data_engine)
                    .map_err(|_| anyhow::anyhow!("data engine is still shared"))?
                    .Close(&encode_context)?;
                importer
                    .ImportAndCleanup(&encode_context, &closed_data)
                    .map_err(|error| anyhow::anyhow!(error))?;
                let closed_index = Arc::try_unwrap(index_engine)
                    .map_err(|_| anyhow::anyhow!("index engine is still shared"))?
                    .Close(&encode_context)?;
                importer
                    .ImportAndCleanup(&encode_context, &closed_index)
                    .map_err(|error| anyhow::anyhow!(error))?;
                Ok(())
            })();
            if let Err(error) = import_result {
                importer.CleanupAllLocalEngines(&EncodeContext::default());
                return Err(error);
            }
        }
        if self.task_meta.Plan.IsGlobalSort() {
            step_meta.BaseExternalMeta.ExternalPath =
                astersql_ingestor_globalsort::SubtaskMetaPath(self.task_id, subtask.ID);
            let external_path = step_meta.BaseExternalMeta.ExternalPath.clone();
            step_meta.BaseExternalMeta.ExternalPath.clear();
            let external = step_meta.Marshal()?;
            step_meta.BaseExternalMeta.ExternalPath = external_path.clone();
            object_store
                .WriteFile(&ObjectContext::default(), &external_path, &external)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        }
        subtask.Meta = step_meta.Marshal()?;
        Ok(())
    }
}

impl execute::StepExecutor for EncodeSortStepExecutor {
    fn Init(&mut self, context: execute::Context) -> anyhow::Result<()> {
        if context.is_cancelled() {
            return Err(anyhow::anyhow!("encode sort cancelled"));
        }
        if self.parent_importer.is_none() && self.runtime.WorkerFactory.is_none() {
            self.parent_importer = Some(self.build_parent_importer(0)?);
        }
        Ok(())
    }
    fn RunSubtask(
        &mut self,
        context: execute::Context,
        subtask: &mut astersql_dxf_framework_proto::subtask::Subtask,
    ) -> anyhow::Result<()> {
        self.run(context, subtask).map_err(normalizeSubtaskErr)
    }
    fn RealtimeSummary(&mut self) -> Option<&execute::SubtaskSummary> {
        self.summary.Update();
        Some(&self.summary)
    }
    fn ResetSummary(&mut self) {
        self.summary.Reset();
    }
    fn Cleanup(&mut self, _: execute::Context) -> anyhow::Result<()> {
        if let Some(mut importer) = self.parent_importer.take() {
            importer.Close();
        }
        Ok(())
    }
    fn TaskMetaModified(&mut self, _: execute::Context, meta: Vec<u8>) -> anyhow::Result<()> {
        self.task_meta = TaskMeta::Unmarshal(&meta)?;
        Ok(())
    }
    fn ResourceModified(
        &mut self,
        _: execute::Context,
        resource: &StepResource,
    ) -> anyhow::Result<()> {
        execute::StepExecFrameworkInfo::SetResource(
            self,
            Arc::new(StepResource {
                CPU: astersql_dxf_framework_proto::subtask::NewAllocatable(resource.CPU.Capacity()),
                Mem: astersql_dxf_framework_proto::subtask::NewAllocatable(resource.Mem.Capacity()),
            }),
        );
        Ok(())
    }
    fn SetFrameworkInfo(&mut self, info: execute::FrameworkInfo) {
        self.framework = Some(info);
    }
}

/// Dispatch the conflict-resolution step from a persisted import task.
/// Go's GetStepExecutor parses task metadata before selecting the step.
pub fn GetConflictResolutionStepExecutor(
    task: &Task,
    runtime: Arc<dyn crate::conflict_resolution::ConflictResolutionRuntime>,
) -> Result<Box<dyn execute::StepExecutor>, errors::SharedError> {
    let meta = TaskMeta::Unmarshal(&task.Meta)?;
    if task.Step != step::ImportStepConflictResolution {
        return Err(errors::New(format!(
            "unknown step {} for import task {}",
            task.Step, task.ID
        )));
    }
    Ok(Box::new(
        crate::conflict_resolution::NewConflictResolutionStepExecutor(task.ID, meta, runtime),
    ))
}

/// Step constructors receive the task runtime's store directly. The runtime
/// owns this handle; no per-keyspace lookup is performed during dispatch.
pub trait ImportStepHost: Send + Sync {
    fn TaskStore(&self) -> Arc<dyn astersql_kv::Storage + Send + Sync>;
    fn NewWriteIngestBackend(
        &self,
        task: &Task,
        meta: &TaskMeta,
        store: Arc<dyn astersql_kv::Storage + Send + Sync>,
    ) -> Result<Arc<dyn WriteIngestBackend>, errors::SharedError>;
    fn NewStepExecutor(
        &self,
        step: step::Step,
        task: &Task,
        meta: &TaskMeta,
        store: Arc<dyn astersql_kv::Storage + Send + Sync>,
    ) -> Result<Box<dyn execute::StepExecutor>, errors::SharedError>;
}

/// Go's importExecutor.GetStepExecutor first decodes task metadata, then
/// dispatches all seven IMPORT INTO stages using the bound task runtime store.
pub fn GetImportStepExecutor(
    task: &Task,
    encode_runtime: Arc<ConfiguredEncodeSortRuntime>,
    host: &dyn ImportStepHost,
) -> Result<Box<dyn execute::StepExecutor>, errors::SharedError> {
    let meta = TaskMeta::Unmarshal(&task.Meta)?;
    match task.Step {
        step::ImportStepEncodeAndSort => Ok(Box::new(NewEncodeSortStepExecutor(
            task.ID,
            meta,
            encode_runtime,
        ))),
        step::ImportStepMergeSort => Ok(Box::new(NewMergeSortStepExecutor(
            task.ID,
            meta,
            encode_runtime,
        ))),
        step::ImportStepWriteAndIngest => {
            let backend = host.NewWriteIngestBackend(task, &meta, host.TaskStore())?;
            Ok(Box::new(NewWriteAndIngestStepExecutor(
                task.ID,
                meta,
                encode_runtime,
                backend,
            )))
        }
        step::ImportStepImport
        | step::ImportStepPostProcess
        | step::ImportStepCollectConflicts
        | step::ImportStepConflictResolution => {
            host.NewStepExecutor(task.Step, task, &meta, host.TaskStore())
        }
        _ => Err(errors::New(format!(
            "unknown step {} for import task {}",
            task.Step, task.ID
        ))),
    }
}

pub(crate) struct MergeStoreAdapter(pub(crate) StorageRef);

impl globalsort::Storage for MergeStoreAdapter {
    fn open(&self, path: &str) -> globalsort::Result<Box<dyn std::io::Read>> {
        crate::write_ingest_backend::open_object_stream(&self.0, path, 0)
    }
    fn open_at(&self, path: &str, offset: u64) -> globalsort::Result<Box<dyn std::io::Read>> {
        crate::write_ingest_backend::open_object_stream(&self.0, path, offset)
    }
    fn record_format(&self) -> globalsort::RecordFormat {
        globalsort::RecordFormat::GoBigEndian64
    }
    fn file_size(&self, path: &str) -> globalsort::Result<u64> {
        let mut reader = self
            .0
            .Open(&ObjectContext::default(), path, None)
            .map_err(|e| globalsort::Error::InvalidData(e.to_string()))?;
        let size = std::io::Seek::seek(&mut reader, std::io::SeekFrom::End(0));
        let close = reader.Close();
        let size = size.map_err(|e| globalsort::Error::InvalidData(e.to_string()))?;
        close.map_err(|e| globalsort::Error::InvalidData(e.to_string()))?;
        Ok(size)
    }
    fn read(&self, path: &str) -> globalsort::Result<Vec<u8>> {
        self.0
            .ReadFile(&ObjectContext::default(), path)
            .map_err(|e| globalsort::Error::InvalidData(e.to_string()))
    }
    fn write(&self, path: &str, value: Vec<u8>) -> globalsort::Result<()> {
        self.0
            .WriteFile(&ObjectContext::default(), path, &value)
            .map_err(|error| globalsort::Error::InvalidData(error.to_string()))
    }
    fn delete_files(&self, paths: &[String]) -> globalsort::Result<()> {
        self.0
            .DeleteFiles(&ObjectContext::default(), paths)
            .map_err(|error| globalsort::Error::InvalidData(error.to_string()))
    }
    fn list_prefix(&self, prefix: &str) -> globalsort::Result<Vec<String>> {
        let mut paths = Vec::new();
        self.0
            .WalkDir(&ObjectContext::default(), None, &mut |path, _| {
                if path.starts_with(prefix) {
                    paths.push(path.to_owned());
                }
                Ok(())
            })
            .map_err(|error| globalsort::Error::InvalidData(error.to_string()))?;
        Ok(paths)
    }
}

// Planner external metadata contains only the bulky fields. Keep the inline
// KV group, timestamp and other fields while loading those external fields.
fn mergeExternalMeta(inline: &[u8], external: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut value: serde_json::Value = serde_json::from_slice(inline)?;
    let external: serde_json::Value = serde_json::from_slice(external)?;
    let fields = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("subtask meta must be an object"))?;
    let external = external
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("external meta must be an object"))?;
    fields.extend(
        external
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    Ok(serde_json::to_vec(&value)?)
}

pub(crate) fn readMergeSortMeta(
    bytes: &[u8],
    store: &StorageRef,
) -> anyhow::Result<MergeSortStepMeta> {
    fn decode(bytes: &[u8]) -> anyhow::Result<MergeSortStepMeta> {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        let mut meta = MergeSortStepMeta::default();
        meta.BaseExternalMeta.ExternalPath = value
            .get("ExternalPath")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .into();
        meta.KVGroup = value
            .get("kv-group")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .into();
        if let Some(files) = value.get("data-files").filter(|files| !files.is_null()) {
            meta.DataFiles = serde_json::from_value(files.clone())?;
        }
        meta.SortedKVMeta = serde_json::from_value(value.clone())?;
        meta.RecordedConflictKVCount = value
            .get("recorded-conflict-kv-count")
            .and_then(|item| item.as_u64())
            .unwrap_or(0);
        Ok(meta)
    }
    let inline = decode(bytes)?;
    if inline.BaseExternalMeta.ExternalPath.is_empty() {
        return Ok(inline);
    }
    let external = store.ReadFile(
        &ObjectContext::default(),
        &inline.BaseExternalMeta.ExternalPath,
    )?;
    let mut meta = decode(&mergeExternalMeta(bytes, &external)?)?;
    meta.BaseExternalMeta.ExternalPath = inline.BaseExternalMeta.ExternalPath;
    Ok(meta)
}

pub struct MergeSortStepExecutor {
    task_id: i64,
    task_meta: TaskMeta,
    runtime: Arc<ConfiguredEncodeSortRuntime>,
    summary: execute::SubtaskSummary,
    framework: Option<execute::FrameworkInfo>,
}

pub fn NewMergeSortStepExecutor(
    task_id: i64,
    task_meta: TaskMeta,
    runtime: Arc<ConfiguredEncodeSortRuntime>,
) -> MergeSortStepExecutor {
    MergeSortStepExecutor {
        task_id,
        task_meta,
        runtime,
        summary: execute::SubtaskSummary::default(),
        framework: None,
    }
}

impl execute::StepExecFrameworkInfo for MergeSortStepExecutor {
    fn restricted(&self) {}
    fn GetStep(&self) -> step::Step {
        self.framework
            .as_ref()
            .map_or(0, execute::StepExecFrameworkInfo::GetStep)
    }
    fn GetResource(&self) -> Option<Arc<StepResource>> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetResource)
    }
    fn SetResource(&self, resource: Arc<StepResource>) {
        if let Some(info) = &self.framework {
            info.SetResource(resource);
        }
    }
    fn GetMeterRecorder(&self) -> Option<Arc<metering::Recorder>> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetMeterRecorder)
    }
    fn GetCheckpointUpdateFunc(&self) -> Option<execute::CheckpointUpdateFunc> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetCheckpointUpdateFunc)
    }
    fn GetCheckpointFunc(&self) -> Option<execute::CheckpointGetFunc> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetCheckpointFunc)
    }
}

impl execute::StepExecutor for MergeSortStepExecutor {
    fn Init(&mut self, _: execute::Context) -> anyhow::Result<()> {
        let resource = execute::StepExecFrameworkInfo::GetResource(self)
            .ok_or_else(|| anyhow::anyhow!("merge sort resource is unavailable"))?;
        Ok(())
    }
    fn RunSubtask(
        &mut self,
        context: execute::Context,
        subtask: &mut astersql_dxf_framework_proto::subtask::Subtask,
    ) -> anyhow::Result<()> {
        let result = (|| -> anyhow::Result<()> {
            if context.is_cancelled() {
                return Err(anyhow::anyhow!("merge sort cancelled"));
            }
            let object_store = if let Some(factory) = &self.runtime.ObjectStoreFactory {
                factory().map_err(|error| anyhow::anyhow!(error))?
            } else {
                self.runtime.ObjectStore.clone()
            };
            struct CloseGuard(StorageRef, bool);
            impl Drop for CloseGuard {
                fn drop(&mut self) {
                    if self.1 {
                        self.0.Close();
                    }
                }
            }
            let _guard = CloseGuard(
                object_store.clone(),
                self.runtime.ObjectStoreFactory.is_some(),
            );
            let _requests = self
                .runtime
                .ObjectStoreFactory
                .as_ref()
                .map(|_| SubtaskRequestRecorder::new(object_store.clone(), &self.summary));
            let mut meta = readMergeSortMeta(&subtask.Meta, &object_store)?;
            let resource = execute::StepExecFrameworkInfo::GetResource(self)
                .ok_or_else(|| anyhow::anyhow!("merge sort resource is unavailable"))?;
            let concurrency = resource.CPU.Capacity().max(1) as usize;
            let memory_per_core = resource.MemoryPerCore();
            let indices = self
                .task_meta
                .Plan
                .DesiredTableInfo
                .as_deref()
                .or(self.task_meta.Plan.TableInfo.as_deref())
                .map(importer::GetIndicesGenKV)
                .unwrap_or_default();
            let on_dup = getOnDupForKVGroup(
                &indices,
                &meta.KVGroup,
                self.task_meta.Plan.GetOnDupKeyMode(),
            )?;
            let merged = Arc::new(Mutex::new(SortedKVMeta::default()));
            let on_close: globalsort::merge::OnWriterClose = Arc::new({
                let merged = merged.clone();
                move |summary| {
                    let converted = WriterSummary {
                        Min: Some(summary.min.clone()),
                        Max: Some(summary.max.clone()),
                        TotalSize: summary.total_size,
                        TotalCnt: summary.total_count,
                        MultipleFilesStats: summary
                            .multiple_files_stats
                            .iter()
                            .map(|stat| MultipleFilesStat {
                                MinKey: summary.min.clone(),
                                MaxKey: summary.max.clone(),
                                Filenames: stat
                                    .filenames
                                    .iter()
                                    .map(|pair| [pair.data_file.clone(), pair.stat_file.clone()])
                                    .collect(),
                                ..Default::default()
                            })
                            .collect(),
                        ConflictInfo: engineapi::ConflictInfo {
                            Count: summary.conflict_info.count,
                            Files: summary.conflict_info.files.clone(),
                        },
                    };
                    merged.lock().unwrap().MergeSummary(&converted);
                }
            });
            let adapter: Arc<dyn globalsort::Storage> =
                Arc::new(MergeStoreAdapter(object_store.clone()));
            let merge_summary = Arc::new(globalsort::merge::SubtaskSummary::default());
            let collector: Arc<dyn globalsort::merge::Collector> = Arc::new(
                globalsort::merge::NewMergeCollector(Some(merge_summary.clone())),
            );
            let operator = globalsort::merge::NewMergeOperator(
                globalsort::reader::CancellationToken::default(),
                adapter,
                memory_per_core,
                crate::encode_and_sort_operator::subtaskPrefix(self.task_id, subtask.ID),
                astersql_ingestor_simplesst::onefile_writer::DefaultOneWriterBlockSize,
                Some(on_close),
                Some(collector),
                concurrency,
                false,
                match on_dup.0 {
                    1 => globalsort::OnDuplicateKey::Record,
                    2 => globalsort::OnDuplicateKey::Remove,
                    3 => globalsort::OnDuplicateKey::Error,
                    _ => globalsort::OnDuplicateKey::Ignore,
                },
            )
            .map_err(|error| anyhow::anyhow!(error))?;
            globalsort::merge::MergeOverlappingFiles(&meta.DataFiles, &operator)
                .map_err(|error| normalizeSubtaskErr(anyhow::anyhow!(error)))?;
            self.summary.Processed.fetch_add(
                merge_summary.processed.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.summary.RowCnt.fetch_add(
                merge_summary.row_count.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            meta.SortedKVMeta = merged.lock().unwrap().clone();
            meta.RecordedConflictKVCount = meta.SortedKVMeta.ConflictInfo.Count;
            let path = globalsort::SubtaskMetaPath(self.task_id, subtask.ID);
            meta.BaseExternalMeta.ExternalPath.clear();
            let external = meta.Marshal()?;
            object_store.WriteFile(&ObjectContext::default(), &path, &external)?;
            meta.BaseExternalMeta.ExternalPath = path;
            subtask.Meta = meta.Marshal()?;
            Ok(())
        })();
        result.map_err(normalizeSubtaskErr)
    }
    fn RealtimeSummary(&mut self) -> Option<&execute::SubtaskSummary> {
        self.summary.Update();
        Some(&self.summary)
    }
    fn ResetSummary(&mut self) {
        self.summary.Reset();
    }
    fn Cleanup(&mut self, _: execute::Context) -> anyhow::Result<()> {
        Ok(())
    }
    fn TaskMetaModified(&mut self, _: execute::Context, bytes: Vec<u8>) -> anyhow::Result<()> {
        self.task_meta = TaskMeta::Unmarshal(&bytes)?;
        Ok(())
    }
    fn ResourceModified(
        &mut self,
        _: execute::Context,
        resource: &StepResource,
    ) -> anyhow::Result<()> {
        execute::StepExecFrameworkInfo::SetResource(
            self,
            Arc::new(StepResource {
                CPU: astersql_dxf_framework_proto::subtask::NewAllocatable(resource.CPU.Capacity()),
                Mem: astersql_dxf_framework_proto::subtask::NewAllocatable(resource.Mem.Capacity()),
            }),
        );
        Ok(())
    }
    fn SetFrameworkInfo(&mut self, info: execute::FrameworkInfo) {
        self.framework = Some(info);
    }
}

/// Exact external-engine request assembled by the Go write-and-ingest step.
#[derive(Clone)]
pub struct WriteIngestRequest {
    pub SubtaskID: i64,
    pub KVGroup: String,
    pub TS: u64,
    pub DataFiles: Vec<String>,
    pub StatFiles: Vec<String>,
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    pub JobKeys: Vec<Vec<u8>>,
    pub SplitKeys: Vec<Vec<u8>>,
    pub TotalFileSize: i64,
    pub TotalKVCount: i64,
    pub MemCapacity: i64,
    pub OnDup: engineapi::OnDuplicateKey,
    pub FilePrefix: String,
}

/// The Lightning backend boundary. Implementations perform physical SST
/// import; the executor retains metadata, error, and resource sequencing.
pub trait WriteIngestBackend: Send + Sync {
    /// Bind the dedicated subtask cloud handle used for request recording.
    fn BindObjectStore(&self, _: StorageRef) {}

    fn SetCollector(&self, collector: Arc<dyn execute::Collector + Send + Sync>);
    fn CloseExternalEngine(&self, request: &WriteIngestRequest) -> anyhow::Result<()>;
    fn ImportEngine(&self, subtask_id: i64, split_size: i64, split_keys: i64)
    -> anyhow::Result<()>;
    fn GetExternalEngineConflictInfo(&self, subtask_id: i64) -> engineapi::ConflictInfo;
    fn CleanupEngine(&self, subtask_id: i64) -> anyhow::Result<()>;
    fn Close(&self);
}

pub(crate) fn readWriteIngestMeta(
    bytes: &[u8],
    store: &StorageRef,
) -> anyhow::Result<(WriteIngestStepMeta, bool)> {
    fn decode(bytes: &[u8]) -> anyhow::Result<(WriteIngestStepMeta, bool)> {
        use base64::Engine as _;
        fn keys(value: Option<&serde_json::Value>) -> anyhow::Result<Vec<Vec<u8>>> {
            let Some(value) = value.filter(|value| !value.is_null()) else {
                return Ok(Vec::new());
            };
            let array = value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("range keys must be an array"))?;
            array
                .iter()
                .map(|item| {
                    if let Some(encoded) = item.as_str() {
                        Ok(base64::engine::general_purpose::STANDARD.decode(encoded)?)
                    } else {
                        Ok(serde_json::from_value::<Vec<u8>>(item.clone())?)
                    }
                })
                .collect()
        }
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        let has_job_keys = value
            .get("range-job-keys")
            .is_some_and(|keys| !keys.is_null());
        let mut meta = WriteIngestStepMeta::default();
        meta.BaseExternalMeta.ExternalPath = value
            .get("ExternalPath")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .into();
        meta.KVGroup = value
            .get("kv-group")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .into();
        meta.TS = value.get("ts").and_then(|item| item.as_u64()).unwrap_or(0);
        if let Some(files) = value.get("data-files").filter(|files| !files.is_null()) {
            meta.DataFiles = serde_json::from_value(files.clone())?;
        }
        if let Some(files) = value.get("stat-files").filter(|files| !files.is_null()) {
            meta.StatFiles = serde_json::from_value(files.clone())?;
        }
        if let Some(sorted) = value
            .get("sorted-kv-meta")
            .filter(|sorted| !sorted.is_null())
        {
            meta.SortedKVMeta = serde_json::from_value(sorted.clone())?;
        }
        meta.RangeJobKeys = keys(value.get("range-job-keys"))?;
        meta.RangeSplitKeys = keys(value.get("range-split-keys"))?;
        meta.RecordedConflictKVCount = value
            .get("recorded-conflict-kv-count")
            .and_then(|item| item.as_u64())
            .unwrap_or(0);
        Ok((meta, has_job_keys))
    }
    let (inline, inline_has_job_keys) = decode(bytes)?;
    if inline.BaseExternalMeta.ExternalPath.is_empty() {
        return Ok((inline, inline_has_job_keys));
    }
    let external = store.ReadFile(
        &ObjectContext::default(),
        &inline.BaseExternalMeta.ExternalPath,
    )?;
    let (mut meta, has_job_keys) = decode(&mergeExternalMeta(bytes, &external)?)?;
    meta.BaseExternalMeta.ExternalPath = inline.BaseExternalMeta.ExternalPath;
    Ok((meta, has_job_keys))
}

pub struct WriteAndIngestStepExecutor {
    task_id: i64,
    task_meta: TaskMeta,
    runtime: Arc<ConfiguredEncodeSortRuntime>,
    backend: Arc<dyn WriteIngestBackend>,
    summary: execute::SubtaskSummary,
    framework: Option<execute::FrameworkInfo>,
}

pub fn NewWriteAndIngestStepExecutor(
    task_id: i64,
    task_meta: TaskMeta,
    runtime: Arc<ConfiguredEncodeSortRuntime>,
    backend: Arc<dyn WriteIngestBackend>,
) -> WriteAndIngestStepExecutor {
    WriteAndIngestStepExecutor {
        task_id,
        task_meta,
        runtime,
        backend,
        summary: execute::SubtaskSummary::default(),
        framework: None,
    }
}

impl execute::StepExecFrameworkInfo for WriteAndIngestStepExecutor {
    fn restricted(&self) {}
    fn GetStep(&self) -> step::Step {
        self.framework
            .as_ref()
            .map_or(0, execute::StepExecFrameworkInfo::GetStep)
    }
    fn GetResource(&self) -> Option<Arc<StepResource>> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetResource)
    }
    fn SetResource(&self, resource: Arc<StepResource>) {
        if let Some(info) = &self.framework {
            info.SetResource(resource);
        }
    }
    fn GetMeterRecorder(&self) -> Option<Arc<metering::Recorder>> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetMeterRecorder)
    }
    fn GetCheckpointUpdateFunc(&self) -> Option<execute::CheckpointUpdateFunc> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetCheckpointUpdateFunc)
    }
    fn GetCheckpointFunc(&self) -> Option<execute::CheckpointGetFunc> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetCheckpointFunc)
    }
}

impl execute::StepExecutor for WriteAndIngestStepExecutor {
    fn Init(&mut self, context: execute::Context) -> anyhow::Result<()> {
        if context.is_cancelled() {
            return Err(anyhow::anyhow!("write ingest cancelled"));
        }
        Ok(())
    }
    fn RunSubtask(
        &mut self,
        context: execute::Context,
        subtask: &mut astersql_dxf_framework_proto::subtask::Subtask,
    ) -> anyhow::Result<()> {
        let result = (|| -> anyhow::Result<()> {
            if context.is_cancelled() {
                return Err(anyhow::anyhow!("write ingest cancelled"));
            }
            let object_store = if let Some(factory) = &self.runtime.ObjectStoreFactory {
                factory().map_err(|error| anyhow::anyhow!(error))?
            } else {
                self.runtime.ObjectStore.clone()
            };
            struct CloseGuard(StorageRef, bool);
            impl Drop for CloseGuard {
                fn drop(&mut self) {
                    if self.1 {
                        self.0.Close();
                    }
                }
            }
            let _guard = CloseGuard(
                object_store.clone(),
                self.runtime.ObjectStoreFactory.is_some(),
            );
            let _requests = self
                .runtime
                .ObjectStoreFactory
                .as_ref()
                .map(|_| SubtaskRequestRecorder::new(object_store.clone(), &self.summary));
            self.backend.BindObjectStore(object_store.clone());
            let (mut meta, has_job_keys) = readWriteIngestMeta(&subtask.Meta, &object_store)?;
            let resource = execute::StepExecFrameworkInfo::GetResource(self)
                .ok_or_else(|| anyhow::anyhow!("write ingest resource is unavailable"))?;
            let indices = self
                .task_meta
                .Plan
                .DesiredTableInfo
                .as_deref()
                .or(self.task_meta.Plan.TableInfo.as_deref())
                .map(importer::GetIndicesGenKV)
                .unwrap_or_default();
            let on_dup = getOnDupForKVGroup(
                &indices,
                &meta.KVGroup,
                self.task_meta.Plan.GetOnDupKeyMode(),
            )?;
            let request = WriteIngestRequest {
                SubtaskID: subtask.ID,
                KVGroup: meta.KVGroup.clone(),
                TS: meta.TS,
                DataFiles: meta.DataFiles.clone(),
                StatFiles: meta.StatFiles.clone(),
                StartKey: meta.SortedKVMeta.StartKey.clone(),
                EndKey: meta.SortedKVMeta.EndKey.clone(),
                JobKeys: if has_job_keys {
                    meta.RangeJobKeys.clone()
                } else {
                    meta.RangeSplitKeys.clone()
                },
                SplitKeys: meta.RangeSplitKeys.clone(),
                TotalFileSize: meta.SortedKVMeta.TotalKVSize as i64,
                TotalKVCount: meta.SortedKVMeta.TotalKVCnt as i64,
                MemCapacity: resource.Mem.Capacity(),
                OnDup: on_dup,
                FilePrefix: crate::encode_and_sort_operator::subtaskPrefix(
                    self.task_id,
                    subtask.ID,
                ),
            };
            let collector_summary = Arc::new(execute::SubtaskSummary::default());
            self.backend.SetCollector(Arc::new(ingestCollector {
                summary: collector_summary.clone(),
                kvGroup: meta.KVGroup.clone(),
                meterRec: execute::StepExecFrameworkInfo::GetMeterRecorder(self)
                    .unwrap_or_else(|| Arc::new(metering::Recorder::default())),
            }));
            self.backend
                .CloseExternalEngine(&request)
                .map_err(normalizeSubtaskErr)?;
            self.backend
                .ImportEngine(subtask.ID, 96 * 1024 * 1024, 960_000)
                .map_err(normalizeSubtaskErr)?;
            self.summary.Processed.fetch_add(
                collector_summary.Processed.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.summary.RowCnt.fetch_add(
                collector_summary.RowCnt.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            let conflict = self.backend.GetExternalEngineConflictInfo(subtask.ID);
            let _ = self.backend.CleanupEngine(subtask.ID);
            if conflict.Count == 0 {
                return Ok(());
            }
            meta.SortedKVMeta.ConflictInfo = conflict.clone();
            meta.RecordedConflictKVCount = conflict.Count;
            let path = globalsort::SubtaskMetaPath(self.task_id, subtask.ID);
            meta.BaseExternalMeta.ExternalPath.clear();
            let mut external: serde_json::Value = serde_json::from_slice(&meta.Marshal()?)?;
            if !has_job_keys {
                external["range-job-keys"] = serde_json::Value::Null;
            }
            let external = serde_json::to_vec(&external)?;
            object_store.WriteFile(&ObjectContext::default(), &path, &external)?;
            meta.BaseExternalMeta.ExternalPath = path;
            subtask.Meta = meta.Marshal()?;
            Ok(())
        })();
        result.map_err(normalizeSubtaskErr)
    }
    fn RealtimeSummary(&mut self) -> Option<&execute::SubtaskSummary> {
        self.summary.Update();
        Some(&self.summary)
    }
    fn ResetSummary(&mut self) {
        self.summary.Reset();
    }
    fn Cleanup(&mut self, _: execute::Context) -> anyhow::Result<()> {
        self.backend.Close();
        Ok(())
    }
    fn TaskMetaModified(&mut self, _: execute::Context, bytes: Vec<u8>) -> anyhow::Result<()> {
        self.task_meta = TaskMeta::Unmarshal(&bytes)?;
        Ok(())
    }
    fn ResourceModified(
        &mut self,
        _: execute::Context,
        resource: &StepResource,
    ) -> anyhow::Result<()> {
        execute::StepExecFrameworkInfo::SetResource(
            self,
            Arc::new(StepResource {
                CPU: astersql_dxf_framework_proto::subtask::NewAllocatable(resource.CPU.Capacity()),
                Mem: astersql_dxf_framework_proto::subtask::NewAllocatable(resource.Mem.Capacity()),
            }),
        );
        Ok(())
    }
    fn SetFrameworkInfo(&mut self, info: execute::FrameworkInfo) {
        self.framework = Some(info);
    }
}

enum ConflictNodeCommand {
    Init(
        node_executor::Context,
        mpsc::Sender<node_executor::Result<()>>,
    ),
    Run(
        node_executor::Context,
        node_executor::Subtask,
        mpsc::Sender<node_executor::Result<Vec<u8>>>,
    ),
    Summary(mpsc::Sender<u64>),
    Reset,
    Cleanup(mpsc::Sender<node_executor::Result<()>>),
    Stop,
}

/// Own the non-Send importer on one dedicated thread while the node framework
/// calls its Send + Sync StepExecutor interface from arbitrary threads.
pub struct ConflictNodeStepExecutor {
    commands: mpsc::Sender<ConflictNodeCommand>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

fn NodeContextToken(context: &node_executor::Context) -> execute::Context {
    let token = execute::Context::new();
    if context.Done() {
        token.cancel();
    }
    token
}

fn RunWithNodeCancellation(
    context: node_executor::Context,
    run: impl FnOnce(execute::Context) -> anyhow::Result<()>,
) -> node_executor::Result<()> {
    let token = NodeContextToken(&context);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher = {
        let token = token.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                if context.Done() {
                    token.cancel();
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    let result = run(token).map_err(|error| node_executor::ExecutorError(error.to_string()));
    done.store(true, Ordering::Release);
    let _ = watcher.join();
    result
}

pub(crate) fn NodeFrameworkTask(
    task_id: i64,
    slots: i32,
    meta: Vec<u8>,
    current_step: step::Step,
) -> Task {
    use std::time::SystemTime;
    Task {
        TaskBase: astersql_dxf_framework_proto::task::TaskBase {
            ID: task_id,
            Key: String::new(),
            Type: astersql_dxf_framework_proto::r#type::ImportInto,
            State: astersql_dxf_framework_proto::task::TaskStateRunning,
            Step: current_step,
            Priority: 1,
            RequiredSlots: slots,
            TargetScope: String::new(),
            CreateTime: SystemTime::UNIX_EPOCH,
            MaxNodeCount: 1,
            ExtraParams: Default::default(),
            Keyspace: String::new(),
        },
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: meta,
        Error: None,
        ModifyParam: astersql_dxf_framework_proto::modify::ModifyParam {
            PrevState: astersql_dxf_framework_proto::task::TaskStateRunning,
            Modifications: Vec::new(),
        },
    }
}

impl ConflictNodeStepExecutor {
    pub fn new(
        task: &node_executor::Task,
        runtime: Arc<dyn crate::conflict_resolution::ConflictResolutionRuntime>,
    ) -> node_executor::Result<Arc<Self>> {
        TaskMeta::Unmarshal(&task.Meta)
            .map_err(|error| node_executor::ExecutorError(error.to_string()))?;
        let meta_bytes = task.Meta.clone();
        let task_id = task.TaskBase.ID;
        let slots = task.TaskBase.RequiredSlots;
        let (commands, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let meta = TaskMeta::Unmarshal(&meta_bytes).expect("validated import task metadata");
            let mut step_executor = crate::conflict_resolution::NewConflictResolutionStepExecutor(
                task_id, meta, runtime,
            );
            let framework_task = NodeFrameworkTask(
                task_id,
                slots,
                meta_bytes,
                step::ImportStepConflictResolution,
            );
            execute::SetFrameworkInfo(
                Some(&mut step_executor),
                &framework_task,
                Arc::new(astersql_dxf_framework_proto::subtask::StepResource {
                    CPU: astersql_dxf_framework_proto::subtask::NewAllocatable(slots as i64),
                    Mem: astersql_dxf_framework_proto::subtask::NewAllocatable(0),
                }),
                None,
                None,
            );
            while let Ok(command) = receiver.recv() {
                match command {
                    ConflictNodeCommand::Init(context, reply) => {
                        let _ = reply.send(RunWithNodeCancellation(context, |token| {
                            execute::StepExecutor::Init(&mut step_executor, token)
                        }));
                    }
                    ConflictNodeCommand::Run(context, subtask, reply) => {
                        let mut proto_subtask = astersql_dxf_framework_proto::subtask::NewSubtask(
                            step::ImportStepConflictResolution,
                            task_id,
                            astersql_dxf_framework_proto::r#type::ImportInto,
                            subtask.SubtaskBase.ExecID,
                            slots,
                            subtask.Meta,
                            1,
                        );
                        let result = RunWithNodeCancellation(context, |token| {
                            execute::StepExecutor::RunSubtask(
                                &mut step_executor,
                                token,
                                &mut proto_subtask,
                            )
                        })
                        .map(|_| proto_subtask.Meta);
                        let _ = reply.send(result);
                    }
                    ConflictNodeCommand::Summary(reply) => {
                        let processed = execute::StepExecutor::RealtimeSummary(&mut step_executor)
                            .map(|summary| summary.Processed.load(Ordering::Relaxed).max(0) as u64)
                            .unwrap_or(0);
                        let _ = reply.send(processed);
                    }
                    ConflictNodeCommand::Reset => {
                        execute::StepExecutor::ResetSummary(&mut step_executor)
                    }
                    ConflictNodeCommand::Cleanup(reply) => {
                        let result = execute::StepExecutor::Cleanup(
                            &mut step_executor,
                            execute::Context::new(),
                        )
                        .map_err(|error| node_executor::ExecutorError(error.to_string()));
                        let _ = reply.send(result);
                        break;
                    }
                    ConflictNodeCommand::Stop => {
                        let _ = execute::StepExecutor::Cleanup(
                            &mut step_executor,
                            execute::Context::new(),
                        );
                        break;
                    }
                }
            }
        });
        Ok(Arc::new(Self {
            commands,
            worker: Mutex::new(Some(worker)),
        }))
    }

    fn request(
        &self,
        command: impl FnOnce(mpsc::Sender<node_executor::Result<()>>) -> ConflictNodeCommand,
    ) -> node_executor::Result<()> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(command(reply))
            .map_err(|_| node_executor::ExecutorError("conflict worker stopped".to_owned()))?;
        response
            .recv()
            .map_err(|_| node_executor::ExecutorError("conflict worker stopped".to_owned()))?
    }
}

impl node_executor::StepExecutor for ConflictNodeStepExecutor {
    fn Init(&self, context: &node_executor::Context) -> node_executor::Result<()> {
        self.request(|reply| ConflictNodeCommand::Init(context.clone(), reply))
    }
    fn RunSubtask(
        &self,
        context: &node_executor::Context,
        subtask: &mut node_executor::Subtask,
    ) -> node_executor::Result<()> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(ConflictNodeCommand::Run(
                context.clone(),
                subtask.clone(),
                reply,
            ))
            .map_err(|_| node_executor::ExecutorError("conflict worker stopped".to_owned()))?;
        subtask.Meta = response
            .recv()
            .map_err(|_| node_executor::ExecutorError("conflict worker stopped".to_owned()))??;
        Ok(())
    }
    fn RealtimeSummary(&self) -> Option<node_executor::SubtaskSummary> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(ConflictNodeCommand::Summary(reply))
            .ok()?;
        Some(node_executor::SubtaskSummary {
            RowCount: response.recv().ok()?,
        })
    }
    fn ResetSummary(&self) {
        let _ = self.commands.send(ConflictNodeCommand::Reset);
    }
    fn Cleanup(&self, _: &node_executor::Context) -> node_executor::Result<()> {
        let result = self.request(ConflictNodeCommand::Cleanup);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        result
    }
}

impl Drop for ConflictNodeStepExecutor {
    fn drop(&mut self) {
        let _ = self.commands.send(ConflictNodeCommand::Stop);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

/// Compose conflict-resolution with the existing import task extension so
/// every other import step retains its current dispatch and retry policy.
pub struct ImportConflictExtension {
    pub Runtime: Arc<dyn crate::conflict_resolution::ConflictResolutionRuntime>,
    pub OtherSteps: Arc<dyn node_executor::Extension>,
}

impl node_executor::Extension for ImportConflictExtension {
    fn IsIdempotent(&self, _: &node_executor::Subtask) -> bool {
        true
    }
    fn GetStepExecutor(
        &self,
        task: &node_executor::Task,
    ) -> node_executor::Result<Arc<dyn node_executor::StepExecutor>> {
        if task.TaskBase.Step == step::ImportStepConflictResolution as i64 {
            ConflictNodeStepExecutor::new(task, self.Runtime.clone())
                .map(|executor| executor as Arc<dyn node_executor::StepExecutor>)
        } else {
            self.OtherSteps.GetStepExecutor(task)
        }
    }
    fn IsRetryableError(&self, error: &node_executor::ExecutorError) -> bool {
        self.OtherSteps.IsRetryableError(error)
    }
}

pub fn RegisterImportConflictExecutor(
    runtime: Arc<dyn crate::conflict_resolution::ConflictResolutionRuntime>,
    other_steps: Arc<dyn node_executor::Extension>,
) {
    node_executor::RegisterTaskType(
        astersql_dxf_framework_proto::r#type::ImportInto.to_owned(),
        Arc::new(move |context, task, mut param| {
            param.Extension = Arc::new(ImportConflictExtension {
                Runtime: runtime.clone(),
                OtherSteps: other_steps.clone(),
            });
            node_executor::NewBaseTaskExecutor(context, task, param)
                as Arc<dyn node_executor::TaskExecutor>
        }),
    );
}

enum EncodeNodeCommand {
    Init(
        node_executor::Context,
        mpsc::Sender<node_executor::Result<()>>,
    ),
    Run(
        node_executor::Context,
        node_executor::Subtask,
        mpsc::Sender<node_executor::Result<Vec<u8>>>,
    ),
    Summary(mpsc::Sender<u64>),
    SummaryJSON(mpsc::Sender<Option<String>>),
    Reset,
    Cleanup(mpsc::Sender<node_executor::Result<()>>),
    Stop,
}

fn import_summary_json(summary: &execute::SubtaskSummary) -> String {
    serde_json::json!({
        "row_count": summary.RowCnt.load(Ordering::Relaxed),
        "bytes": summary.Processed.load(Ordering::Relaxed),
        "read_bytes": summary.ReadBytes.load(Ordering::Relaxed),
        "get_request_count": summary.GetReqCnt.load(Ordering::Relaxed),
        "put_request_count": summary.PutReqCnt.load(Ordering::Relaxed),
    })
    .to_string()
}

/// Bridges the node framework's Send + Sync interface to a thread-owned
/// import-step executor and returns its updated subtask Meta to FinishSubtask.
pub struct EncodeSortNodeStepExecutor {
    commands: mpsc::Sender<EncodeNodeCommand>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl EncodeSortNodeStepExecutor {
    pub fn new_import(
        task: &node_executor::Task,
        runtime: Arc<ConfiguredEncodeSortRuntime>,
        host: Arc<dyn ImportStepHost>,
        resource: node_executor::StepResource,
    ) -> node_executor::Result<Arc<Self>> {
        TaskMeta::Unmarshal(&task.Meta)
            .map_err(|error| node_executor::ExecutorError(error.to_string()))?;
        let meta_bytes = task.Meta.clone();
        let task_id = task.TaskBase.ID;
        let slots = task.TaskBase.RequiredSlots;
        let current_step = task.TaskBase.Step;
        let (commands, receiver) = mpsc::channel();
        let (ready, ready_response) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let framework_task = NodeFrameworkTask(task_id, slots, meta_bytes, current_step);
            let built = GetImportStepExecutor(&framework_task, runtime, host.as_ref());
            let _ = ready.send(
                built
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| node_executor::ExecutorError(error.to_string())),
            );
            let Ok(mut step_executor) = built else {
                return;
            };
            execute::SetFrameworkInfo(
                Some(step_executor.as_mut()),
                &framework_task,
                Arc::new(StepResource {
                    CPU: astersql_dxf_framework_proto::subtask::NewAllocatable(resource.CPU as i64),
                    Mem: astersql_dxf_framework_proto::subtask::NewAllocatable(resource.Memory),
                }),
                None,
                None,
            );
            while let Ok(command) = receiver.recv() {
                match command {
                    EncodeNodeCommand::Init(context, reply) => {
                        let _ = reply.send(RunWithNodeCancellation(context, |token| {
                            step_executor.Init(token)
                        }));
                    }
                    EncodeNodeCommand::Run(context, subtask, reply) => {
                        let mut proto_subtask = astersql_dxf_framework_proto::subtask::NewSubtask(
                            current_step,
                            task_id,
                            astersql_dxf_framework_proto::r#type::ImportInto,
                            subtask.SubtaskBase.ExecID,
                            slots,
                            subtask.Meta,
                            1,
                        );
                        proto_subtask.SubtaskBase.ID = subtask.SubtaskBase.ID;
                        let result = RunWithNodeCancellation(context, |token| {
                            step_executor.RunSubtask(token, &mut proto_subtask)
                        })
                        .map(|_| proto_subtask.Meta);
                        let _ = reply.send(result);
                    }
                    EncodeNodeCommand::Summary(reply) => {
                        let count = step_executor
                            .RealtimeSummary()
                            .map(|summary| summary.Processed.load(Ordering::Relaxed).max(0) as u64)
                            .unwrap_or(0);
                        let _ = reply.send(count);
                    }
                    EncodeNodeCommand::SummaryJSON(reply) => {
                        let _ =
                            reply.send(step_executor.RealtimeSummary().map(import_summary_json));
                    }
                    EncodeNodeCommand::Reset => step_executor.ResetSummary(),
                    EncodeNodeCommand::Cleanup(reply) => {
                        let result = step_executor
                            .Cleanup(execute::Context::new())
                            .map_err(|error| node_executor::ExecutorError(error.to_string()));
                        let _ = reply.send(result);
                        break;
                    }
                    EncodeNodeCommand::Stop => {
                        let _ = step_executor.Cleanup(execute::Context::new());
                        break;
                    }
                }
            }
        });
        ready_response
            .recv()
            .map_err(|_| node_executor::ExecutorError("import step factory stopped".into()))??;
        Ok(Arc::new(Self {
            commands,
            worker: Mutex::new(Some(worker)),
        }))
    }

    pub fn new(
        task: &node_executor::Task,
        runtime: Arc<ConfiguredEncodeSortRuntime>,
        resource: node_executor::StepResource,
    ) -> node_executor::Result<Arc<Self>> {
        TaskMeta::Unmarshal(&task.Meta)
            .map_err(|error| node_executor::ExecutorError(error.to_string()))?;
        let meta_bytes = task.Meta.clone();
        let task_id = task.TaskBase.ID;
        let slots = task.TaskBase.RequiredSlots;
        let (commands, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let meta = TaskMeta::Unmarshal(&meta_bytes).expect("validated import task metadata");
            let mut step_executor = NewEncodeSortStepExecutor(task_id, meta, runtime);
            let framework_task =
                NodeFrameworkTask(task_id, slots, meta_bytes, step::ImportStepEncodeAndSort);
            execute::SetFrameworkInfo(
                Some(&mut step_executor),
                &framework_task,
                Arc::new(StepResource {
                    CPU: astersql_dxf_framework_proto::subtask::NewAllocatable(resource.CPU as i64),
                    Mem: astersql_dxf_framework_proto::subtask::NewAllocatable(resource.Memory),
                }),
                None,
                None,
            );
            while let Ok(command) = receiver.recv() {
                match command {
                    EncodeNodeCommand::Init(context, reply) => {
                        let _ = reply.send(RunWithNodeCancellation(context, |token| {
                            execute::StepExecutor::Init(&mut step_executor, token)
                        }));
                    }
                    EncodeNodeCommand::Run(context, subtask, reply) => {
                        let mut proto_subtask = astersql_dxf_framework_proto::subtask::NewSubtask(
                            step::ImportStepEncodeAndSort,
                            task_id,
                            astersql_dxf_framework_proto::r#type::ImportInto,
                            subtask.SubtaskBase.ExecID,
                            slots,
                            subtask.Meta,
                            1,
                        );
                        proto_subtask.SubtaskBase.ID = subtask.SubtaskBase.ID;
                        let result = RunWithNodeCancellation(context, |token| {
                            execute::StepExecutor::RunSubtask(
                                &mut step_executor,
                                token,
                                &mut proto_subtask,
                            )
                        })
                        .map(|_| proto_subtask.Meta);
                        let _ = reply.send(result);
                    }
                    EncodeNodeCommand::Summary(reply) => {
                        let count = execute::StepExecutor::RealtimeSummary(&mut step_executor)
                            .map(|summary| summary.Processed.load(Ordering::Relaxed).max(0) as u64)
                            .unwrap_or(0);
                        let _ = reply.send(count);
                    }
                    EncodeNodeCommand::SummaryJSON(reply) => {
                        let _ = reply.send(
                            execute::StepExecutor::RealtimeSummary(&mut step_executor)
                                .map(import_summary_json),
                        );
                    }
                    EncodeNodeCommand::Reset => {
                        execute::StepExecutor::ResetSummary(&mut step_executor)
                    }
                    EncodeNodeCommand::Cleanup(reply) => {
                        let result = execute::StepExecutor::Cleanup(
                            &mut step_executor,
                            execute::Context::new(),
                        )
                        .map_err(|error| node_executor::ExecutorError(error.to_string()));
                        let _ = reply.send(result);
                        break;
                    }
                    EncodeNodeCommand::Stop => {
                        let _ = execute::StepExecutor::Cleanup(
                            &mut step_executor,
                            execute::Context::new(),
                        );
                        break;
                    }
                }
            }
        });
        Ok(Arc::new(Self {
            commands,
            worker: Mutex::new(Some(worker)),
        }))
    }

    fn request(
        &self,
        command: impl FnOnce(mpsc::Sender<node_executor::Result<()>>) -> EncodeNodeCommand,
    ) -> node_executor::Result<()> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(command(reply))
            .map_err(|_| node_executor::ExecutorError("encode sort worker stopped".into()))?;
        response
            .recv()
            .map_err(|_| node_executor::ExecutorError("encode sort worker stopped".into()))?
    }
}

impl node_executor::StepExecutor for EncodeSortNodeStepExecutor {
    fn Init(&self, context: &node_executor::Context) -> node_executor::Result<()> {
        self.request(|reply| EncodeNodeCommand::Init(context.clone(), reply))
    }
    fn RunSubtask(
        &self,
        context: &node_executor::Context,
        subtask: &mut node_executor::Subtask,
    ) -> node_executor::Result<()> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(EncodeNodeCommand::Run(
                context.clone(),
                subtask.clone(),
                reply,
            ))
            .map_err(|_| node_executor::ExecutorError("encode sort worker stopped".into()))?;
        subtask.Meta = response
            .recv()
            .map_err(|_| node_executor::ExecutorError("encode sort worker stopped".into()))??;
        Ok(())
    }
    fn RealtimeSummary(&self) -> Option<node_executor::SubtaskSummary> {
        let (reply, response) = mpsc::channel();
        self.commands.send(EncodeNodeCommand::Summary(reply)).ok()?;
        Some(node_executor::SubtaskSummary {
            RowCount: response.recv().ok()?,
        })
    }
    fn RealtimeSummaryJSON(&self) -> Option<String> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(EncodeNodeCommand::SummaryJSON(reply))
            .ok()?;
        response.recv().ok().flatten()
    }
    fn ResetSummary(&self) {
        let _ = self.commands.send(EncodeNodeCommand::Reset);
    }
    fn Cleanup(&self, _: &node_executor::Context) -> node_executor::Result<()> {
        let result = self.request(EncodeNodeCommand::Cleanup);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        result
    }
}

impl Drop for EncodeSortNodeStepExecutor {
    fn drop(&mut self) {
        let _ = self.commands.send(EncodeNodeCommand::Stop);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

/// Routes encode-and-sort through the node task manager; callers may wrap
/// another import extension (including conflict resolution) as OtherSteps.
pub struct ImportEncodeExtension {
    pub Runtime: Arc<ConfiguredEncodeSortRuntime>,
    pub NodeResource: node_executor::NodeResource,
    pub OtherSteps: Arc<dyn node_executor::Extension>,
}

impl node_executor::Extension for ImportEncodeExtension {
    fn IsIdempotent(&self, _: &node_executor::Subtask) -> bool {
        true
    }
    fn GetStepExecutor(
        &self,
        task: &node_executor::Task,
    ) -> node_executor::Result<Arc<dyn node_executor::StepExecutor>> {
        if task.TaskBase.Step == step::ImportStepEncodeAndSort as i64 {
            EncodeSortNodeStepExecutor::new(
                task,
                self.Runtime.clone(),
                self.NodeResource.GetStepResource(&task.TaskBase),
            )
            .map(|executor| executor as Arc<dyn node_executor::StepExecutor>)
        } else {
            self.OtherSteps.GetStepExecutor(task)
        }
    }
    fn IsRetryableError(&self, error: &node_executor::ExecutorError) -> bool {
        self.OtherSteps.IsRetryableError(error)
    }
}

pub fn RegisterImportEncodeExecutor(
    runtime: Arc<ConfiguredEncodeSortRuntime>,
    other_steps: Arc<dyn node_executor::Extension>,
) {
    node_executor::RegisterTaskType(
        astersql_dxf_framework_proto::r#type::ImportInto.to_owned(),
        Arc::new(move |context, task, mut param| {
            param.Extension = Arc::new(ImportEncodeExtension {
                Runtime: runtime.clone(),
                NodeResource: param.nodeRc.clone(),
                OtherSteps: other_steps.clone(),
            });
            node_executor::NewBaseTaskExecutor(context, task, param)
                as Arc<dyn node_executor::TaskExecutor>
        }),
    );
}

/// Unified IMPORT INTO node extension; the host supplies the already-bound
/// task runtime store and the external services for the remaining stages.
pub struct ImportTaskExtension {
    pub Runtime: Arc<ConfiguredEncodeSortRuntime>,
    pub Host: Arc<dyn ImportStepHost>,
    pub NodeResource: node_executor::NodeResource,
    pub RetryPolicy: Arc<dyn node_executor::Extension>,
}

pub struct ImportNodeTaskExecutor {
    base: Arc<node_executor::BaseTaskExecutor>,
    task_id: i64,
    closed: std::sync::atomic::AtomicBool,
}

impl node_executor::TaskExecutor for ImportNodeTaskExecutor {
    fn Init(&self, context: &node_executor::Context) -> node_executor::Result<()> {
        self.base.Init(context)
    }
    fn Run(&self) {
        self.base.Run();
    }
    fn GetTaskBase(&self) -> node_executor::TaskBase {
        self.base.GetTaskBase()
    }
    fn CancelRunningSubtask(&self) {
        self.base.CancelRunningSubtask();
    }
    fn Cancel(&self) {
        self.base.Cancel();
    }
    fn Close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            crate::metrics::metricsManager.unregister(self.task_id);
            self.base.Close();
        }
    }
    fn IsRetryableError(&self, error: &node_executor::ExecutorError) -> bool {
        self.base.IsRetryableError(error)
    }
}

impl Drop for ImportNodeTaskExecutor {
    fn drop(&mut self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            crate::metrics::metricsManager.unregister(self.task_id);
            self.base.Close();
        }
    }
}

impl node_executor::Extension for ImportTaskExtension {
    fn IsIdempotent(&self, _: &node_executor::Subtask) -> bool {
        true
    }
    fn GetStepExecutor(
        &self,
        task: &node_executor::Task,
    ) -> node_executor::Result<Arc<dyn node_executor::StepExecutor>> {
        EncodeSortNodeStepExecutor::new_import(
            task,
            self.Runtime.clone(),
            self.Host.clone(),
            self.NodeResource.GetStepResource(&task.TaskBase),
        )
        .map(|executor| executor as Arc<dyn node_executor::StepExecutor>)
    }
    fn IsRetryableError(&self, error: &node_executor::ExecutorError) -> bool {
        self.RetryPolicy.IsRetryableError(error)
    }
}

pub fn RegisterImportExecutor(
    runtime: Arc<ConfiguredEncodeSortRuntime>,
    host: Arc<dyn ImportStepHost>,
    retry_policy: Arc<dyn node_executor::Extension>,
) {
    node_executor::RegisterTaskType(
        astersql_dxf_framework_proto::r#type::ImportInto.to_owned(),
        Arc::new(move |context, task, mut param| {
            let task_id = task.TaskBase.ID;
            let _ = crate::metrics::metricsManager.get_or_create_metrics(task_id);
            param.Extension = Arc::new(ImportTaskExtension {
                Runtime: runtime.clone(),
                Host: host.clone(),
                NodeResource: param.nodeRc.clone(),
                RetryPolicy: retry_policy.clone(),
            });
            Arc::new(ImportNodeTaskExecutor {
                base: node_executor::NewBaseTaskExecutor(context, task, param),
                task_id,
                closed: std::sync::atomic::AtomicBool::new(false),
            }) as Arc<dyn node_executor::TaskExecutor>
        }),
    );
}

/// 单个 worker 可用内存中划给 KV writer 的比例。
/// Fraction of the memory available to one worker reserved for KV writers.
pub const writerMemBudgetRatio: f64 = 0.5;
/// 任务总内存中划给 parquet reader 的比例。
/// Fraction of total task memory available to parquet readers.
pub const readerMemBudgetRatio: f64 = 0.3;
/// 默认块大小 16MiB，用于索引 writer 缓冲对齐。
const defaultBlockSize: usize = 16 * 1024 * 1024;
const maxTxnEntrySizeLimit: usize = 120 * 1024 * 1024;

/// 将 writer 缓冲对齐到整块；若对齐浪费超过预算 10% 则保持原大小；零输入仍为零。
/// Align a writer buffer to whole blocks unless alignment would waste more
/// than ten percent of its memory budget. A zero budget retains the Go default.
pub(crate) fn getAdjustedBlockSize(total_buf_size: u64, default_block_size: usize) -> usize {
    if total_buf_size == 0 {
        return default_block_size;
    }
    if default_block_size == 0 {
        return 0;
    }
    let block_size = default_block_size as u64;
    let aligned_size = total_buf_size.div_ceil(block_size) * block_size;
    // 向上对齐后超过原预算 10% 则放弃对齐，避免过度浪费内存。
    if aligned_size as f64 / total_buf_size as f64 > 1.1 {
        total_buf_size as usize
    } else {
        default_block_size
    }
}

/// 编码/导入步骤在 TableImporter 构建后持有的状态。
///
/// `TableImporter` 的构造仍留在集成边界：Rust importer 需要显式的
/// `TableImporterService`；保持该依赖显式可避免旧草稿中的假 session/storage 调用。
/// The state owned by the encode/import step after its importer has been built.
///
/// Construction of `TableImporter` remains at the integration boundary because
/// the Rust importer requires an explicit `TableImporterService`; keeping that
/// dependency explicit avoids the old draft's fake session and storage calls.
pub struct importStepExecutor {
    pub taskID: i64,
    pub tableImporter: importer::TableImporter,
    /// 目标表各索引的 GenKV 描述，用于重复键策略判定。
    pub indicesGenKV: HashMap<i64, importer::GenKVIndex>,
    /// 每连接 data KV writer 内存上限。
    pub dataKVMemSizePerCon: u64,
    /// 每连接每个索引 KV writer 内存上限。
    pub perIndexKVMemSizePerCon: u64,
    pub indexBlockSize: usize,
    pub dataBlockSize: usize,
    pub concurrency: i32,
    pub summary: execute::SubtaskSummary,
}

impl importStepExecutor {
    /// 根据步骤资源与事务条目大小上限初始化执行器内存/并发参数。
    pub fn new(
        taskID: i64,
        tableImporter: importer::TableImporter,
        resource: &StepResource,
        max_txn_entry_size: usize,
    ) -> Self {
        let plan = &tableImporter.LoadDataController.Plan;
        // 优先 DesiredTableInfo，否则回退 TableInfo，用于生成索引 GenKV 映射。
        let indicesGenKV = plan
            .DesiredTableInfo
            .as_deref()
            .or(plan.TableInfo.as_deref())
            .map(importer::GetIndicesGenKV)
            .unwrap_or_default();
        let (dataKVMemSizePerCon, perIndexKVMemSizePerCon) =
            crate::encode_and_sort_operator::getWriterMemorySizeLimit(
                resource,
                &tableImporter.LoadDataController.Plan,
            );
        Self {
            taskID,
            tableImporter,
            indicesGenKV,
            dataKVMemSizePerCon,
            perIndexKVMemSizePerCon,
            dataBlockSize: getAdjustedBlockSize(dataKVMemSizePerCon, max_txn_entry_size),
            indexBlockSize: getAdjustedBlockSize(perIndexKVMemSizePerCon, defaultBlockSize),
            concurrency: resource.CPU.Capacity().max(1) as i32,
            summary: execute::SubtaskSummary::default(),
        }
    }

    /// 对齐 Go 的一次性 parquet 并发估计；调用方保留 one-shot 守卫，因执行器可能收到多个子任务。
    /// Matches Go's one-shot parquet concurrency estimate. Callers retain the
    /// one-shot guard because the executor may receive several subtasks.
    pub fn estimateAndSetConcurrency(
        &mut self,
        chunks: &[importer::Chunk],
        resource: &StepResource,
    ) {
        self.concurrency = parquet_reader_concurrency(
            chunks,
            self.concurrency as usize,
            resource.Mem.Capacity(),
            |path, size| self.tableImporter.EstimateParquetReaderMemory(path, size),
        ) as i32;
    }

    /// 刷新并返回实时子任务进度摘要。
    pub fn RealtimeSummary(&mut self) -> &execute::SubtaskSummary {
        self.summary.Update();
        &self.summary
    }

    /// 重置进度计数。
    pub fn ResetSummary(&mut self) {
        self.summary.Reset();
    }

    /// 关闭 TableImporter，释放导入资源。
    pub fn Cleanup(&mut self) {
        self.tableImporter.Close();
    }
}

impl execute::Collector for importStepExecutor {
    /// 累加已接收字节到 Processed。
    fn Accepted(&self, bytes: i64) {
        self.summary.Processed.fetch_add(bytes, Ordering::Relaxed);
    }

    /// 累加已处理行数到 RowCnt。
    fn Processed(&self, _processed_units: i64, rows: i64) {
        self.summary.RowCnt.fetch_add(rows, Ordering::Relaxed);
    }
}

/// 将 importer 的 OnDupKeyMode 映射为 engineapi 对冲突 KV 的策略。
pub fn getOnDupForConflictedKV(mode: importer::OnDupKeyMode) -> engineapi::OnDuplicateKey {
    if mode == importer::OnDupKeyModeCapture {
        engineapi::OnDuplicateKeyRecord
    } else {
        engineapi::OnDuplicateKeyError
    }
}

/// Go normalizes Lightning's duplicate-key error before returning a subtask failure.
pub fn normalizeSubtaskErr(error: anyhow::Error) -> anyhow::Error {
    fn has_duplicate(error: &lightning_common::CommonError) -> bool {
        error.ID == "Lightning:Restore:ErrFoundDuplicateKey"
            || error.Causes.iter().any(has_duplicate)
    }
    let duplicate = error.chain().any(|cause| {
        let lightning = cause
            .downcast_ref::<lightning_common::CommonError>()
            .is_some_and(has_duplicate);
        let normalized = cause
            .downcast_ref::<errors::SharedError>()
            .and_then(|shared| errors::Cause(Some(shared)))
            .and_then(|root| {
                root.downcast_ref::<errors::Error>()
                    .map(|normalized| normalized.ID())
            })
            .is_some_and(|id| id == "Lightning:Restore:ErrFoundDuplicateKey");
        let sort_duplicate = cause
            .downcast_ref::<globalsort::Error>()
            .is_some_and(|error| matches!(error, globalsort::Error::DuplicateKey { .. }));
        lightning || normalized || sort_duplicate
    });
    if duplicate {
        anyhow::anyhow!(
            astersql_util_dbterror_exeerrors::exeerrors::ErrLoadDataDuplicateKeyConflict
                .FastGenByArgs(&[])
        )
    } else {
        error
    }
}

/// 按 KV 组名解析 data 或 index，并返回对应的重复键策略。
pub fn getOnDupForKVGroup(
    indices: &HashMap<i64, importer::GenKVIndex>,
    kv_group: &str,
    mode: importer::OnDupKeyMode,
) -> Result<engineapi::OnDuplicateKey, errors::SharedError> {
    if kv_group == DATA_KV_GROUP {
        return Ok(getOnDupForConflictedKV(mode));
    }
    // 非 data 组时组名应为 index id 字符串。
    let index_id = kv_group
        .parse::<i64>()
        .map_err(|error| errors::New(error.to_string()))?;
    getOnDupForIndex(indices, index_id, mode)
}

/// 按索引是否唯一与 OnDupKeyMode 决定 Record / Error / Remove。
pub fn getOnDupForIndex(
    indices: &HashMap<i64, importer::GenKVIndex>,
    index_id: i64,
    mode: importer::OnDupKeyMode,
) -> Result<engineapi::OnDuplicateKey, errors::SharedError> {
    let index = indices
        .get(&index_id)
        .ok_or_else(|| errors::New(format!("unknown index {index_id}")))?;
    if mode == importer::OnDupKeyModeError {
        return Ok(engineapi::OnDuplicateKeyError);
    }
    // 唯一索引走冲突记录/报错策略；非唯一索引直接 Remove 重复项。
    if index.Unique {
        Ok(getOnDupForConflictedKV(mode))
    } else {
        Ok(engineapi::OnDuplicateKeyRemove)
    }
}

/// ingest 阶段进度收集器：更新子任务摘要并上报集群写入字节计量。
pub struct ingestCollector {
    pub summary: Arc<execute::SubtaskSummary>,
    /// 当前 ingest 的 KV 组名；仅 data 组计入行数。
    pub kvGroup: String,
    pub meterRec: Arc<metering::Recorder>,
}

impl execute::Collector for ingestCollector {
    fn Accepted(&self, _bytes: i64) {}

    fn Processed(&self, bytes: i64, rows: i64) {
        self.summary.Processed.fetch_add(bytes, Ordering::Relaxed);
        // 索引组不计入导入行数，避免重复累计。
        if self.kvGroup == DATA_KV_GROUP {
            self.summary.RowCnt.fetch_add(rows, Ordering::Relaxed);
        }
        self.meterRec.IncClusterWriteBytes(bytes.max(0) as u64);
    }
}

/// Both framework entrypoints estimate the largest Parquet file with its exact
/// source size. Estimation failures retain CPU-based concurrency.
pub(crate) fn parquet_reader_concurrency(
    chunks: &[importer::Chunk],
    cpu: usize,
    memory: i64,
    estimate: impl FnOnce(&str, i64) -> Result<i64, String>,
) -> usize {
    if chunks.first().map(|chunk| chunk.Type) != Some(SourceType::Parquet) {
        return cpu;
    }
    let target = chunks.iter().max_by_key(|chunk| chunk.FileSize).unwrap();
    let Ok(peak) = estimate(&target.Path, target.FileSize) else {
        return cpu;
    };
    if peak <= 0 {
        return cpu;
    }
    let budget = (memory as f64 * readerMemBudgetRatio) as i64;
    cpu.min((budget / peak).max(1) as usize)
}
