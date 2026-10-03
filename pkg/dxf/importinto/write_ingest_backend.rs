// Copyright 2026 AsterSQL.

//! Native global-sort ingestion. Only Region discovery and TiKV RPCs are supplied
//! by the host; loading, retry stages, progress, and engine lifetimes are owned here.

use crate::task_executor::{ImportStepHost, WriteIngestBackend, WriteIngestRequest};
use api::Engine as _;
use astersql_dxf_framework_taskexecutor_execute as execute;
use astersql_ingestor_engineapi as api;
use astersql_ingestor_globalsort as global;
use astersql_ingestor_ingestctrl as local;
use local::job_worker::{RegionJob, RegionJobWorker, TikvWriteResult};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// RPC boundary. Write statistics describe the actual completed write, including
/// a write whose later Ingest fails. Scan must return the regions covering range.
pub trait RegionImportTransport: Send + Sync {
    fn Scan(
        &self,
        token: &local::CancellationToken,
        range: &local::KeyRange,
    ) -> local::Result<Vec<local::region_job::LocatedRegion>>;
    fn Write(
        &self,
        token: &local::CancellationToken,
        job: &RegionJob,
        data: &dyn api::IngestData,
    ) -> local::Result<TikvWriteResult>;
    fn Ingest(&self, token: &local::CancellationToken, job: &RegionJob) -> local::Result<()>;
    fn Close(&self);
}

struct ClosingObjectReader(Box<dyn astersql_objstore::objectio::Reader>);
impl std::io::Read for ClosingObjectReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buffer)
    }
}
impl Drop for ClosingObjectReader {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}

pub(crate) fn open_object_stream(
    store: &astersql_objstore_storeapi::StorageRef,
    path: &str,
    offset: u64,
) -> global::Result<Box<dyn std::io::Read>> {
    let mut reader = store
        .Open(&Default::default(), path, None)
        .map_err(|e| global::Error::InvalidData(e.to_string()))?;
    if let Err(error) = std::io::Seek::seek(&mut reader, std::io::SeekFrom::Start(offset)) {
        let _ = reader.Close();
        return Err(global::Error::InvalidData(error.to_string()));
    }
    Ok(Box::new(ClosingObjectReader(reader)))
}

struct ObjectStore(astersql_objstore_storeapi::StorageRef);
impl global::Storage for ObjectStore {
    fn open(&self, path: &str) -> global::Result<Box<dyn std::io::Read>> {
        open_object_stream(&self.0, path, 0)
    }
    fn open_at(&self, path: &str, offset: u64) -> global::Result<Box<dyn std::io::Read>> {
        open_object_stream(&self.0, path, offset)
    }
    fn record_format(&self) -> global::RecordFormat {
        global::RecordFormat::GoBigEndian64
    }
    fn file_size(&self, path: &str) -> global::Result<u64> {
        let mut reader = self
            .0
            .Open(&Default::default(), path, None)
            .map_err(|e| global::Error::InvalidData(e.to_string()))?;
        let size = std::io::Seek::seek(&mut reader, std::io::SeekFrom::End(0));
        let close = reader.Close();
        let size = size.map_err(|e| global::Error::InvalidData(e.to_string()))?;
        close.map_err(|e| global::Error::InvalidData(e.to_string()))?;
        Ok(size)
    }
    fn read(&self, path: &str) -> global::Result<Vec<u8>> {
        self.0
            .ReadFile(&Default::default(), path)
            .map_err(|e| global::Error::InvalidData(e.to_string()))
    }
    fn write(&self, path: &str, data: Vec<u8>) -> global::Result<()> {
        self.0
            .WriteFile(&Default::default(), path, &data)
            .map_err(|e| global::Error::InvalidData(e.to_string()))
    }
    fn delete_files(&self, paths: &[String]) -> global::Result<()> {
        self.0
            .DeleteFiles(&Default::default(), paths)
            .map_err(|e| global::Error::InvalidData(e.to_string()))
    }
    fn list_prefix(&self, prefix: &str) -> global::Result<Vec<String>> {
        let mut paths = Vec::new();
        self.0
            .WalkDir(&Default::default(), None, &mut |path, _| {
                if path.starts_with(prefix) {
                    paths.push(path.to_owned());
                }
                Ok(())
            })
            .map_err(|e| global::Error::InvalidData(e.to_string()))?;
        Ok(paths)
    }
}

pub struct GlobalSortWriteIngestBackend {
    store: Mutex<astersql_objstore_storeapi::StorageRef>,
    transport: Arc<dyn RegionImportTransport>,
    engines: Mutex<HashMap<i64, Arc<global::engine::ExternalEngineAdapter>>>,
    collector: Mutex<Option<Arc<dyn execute::Collector + Send + Sync>>>,
    token: local::CancellationToken,
    concurrency: usize,
}
impl GlobalSortWriteIngestBackend {
    pub fn new(
        store: astersql_objstore_storeapi::StorageRef,
        transport: Arc<dyn RegionImportTransport>,
        concurrency: usize,
    ) -> Self {
        Self {
            store: Mutex::new(store),
            transport,
            engines: Default::default(),
            collector: Default::default(),
            token: Default::default(),
            concurrency: concurrency.max(1),
        }
    }
}
impl WriteIngestBackend for GlobalSortWriteIngestBackend {
    fn BindObjectStore(&self, store: astersql_objstore_storeapi::StorageRef) {
        *self.store.lock().unwrap() = store;
    }

    fn SetCollector(&self, collector: Arc<dyn execute::Collector + Send + Sync>) {
        *self.collector.lock().unwrap() = Some(collector);
    }
    fn CloseExternalEngine(&self, request: &WriteIngestRequest) -> anyhow::Result<()> {
        let on_dup = match request.OnDup {
            api::OnDuplicateKeyIgnore => global::OnDuplicateKey::Ignore,
            api::OnDuplicateKeyRecord => global::OnDuplicateKey::Record,
            api::OnDuplicateKeyRemove => global::OnDuplicateKey::Remove,
            api::OnDuplicateKeyError => global::OnDuplicateKey::Error,
            _ => anyhow::bail!("unknown duplicate-key mode"),
        };
        let engine = global::engine::NewExternalEngine(
            Arc::new(ObjectStore(self.store.lock().unwrap().clone())),
            request.DataFiles.clone(),
            request.StatFiles.clone(),
            request.StartKey.clone(),
            request.EndKey.clone(),
            request.JobKeys.clone(),
            request.SplitKeys.clone(),
            self.concurrency as i32,
            request.TS,
            request.TotalFileSize,
            request.TotalKVCount,
            false,
            request.MemCapacity,
            on_dup,
            request.FilePrefix.clone(),
        )?;
        let mut engines = self.engines.lock().unwrap();
        if engines.contains_key(&request.SubtaskID) {
            anyhow::bail!("external engine {} already exists", request.SubtaskID);
        }
        engines.insert(
            request.SubtaskID,
            Arc::new(global::engine::ExternalEngineAdapter::new(
                engine,
                Default::default(),
            )),
        );
        Ok(())
    }
    fn ImportEngine(&self, id: i64, split_size: i64, split_keys: i64) -> anyhow::Result<()> {
        let engine = self
            .engines
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("external engine {id} is missing"))?;
        let transport = self.transport.clone();
        let generate: local::import_pipeline::JobGenerator =
            Arc::new(move |token, data, ranges| {
                let mut jobs = Vec::new();
                for range in ranges {
                    let range = local::KeyRange {
                        start: range.Start.clone(),
                        end: range.End.clone(),
                    };
                    let regions = transport.Scan(token, &range)?;
                    let generated = local::region_job::newRegionJobs(
                        &regions,
                        &[],
                        &[range],
                        split_size,
                        split_keys,
                    );
                    if generated.is_empty() {
                        return Err(local::Error::InvalidData(
                            "region scan returned no covering regions".into(),
                        ));
                    }
                    jobs.extend(generated.into_iter().map(|mut job| {
                        job.timestamp = data.GetTS();
                        job
                    }));
                }
                Ok(jobs)
            });
        let transport = self.transport.clone();
        let collector = self
            .collector
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("ingest collector is not set"))?;
        let workers: local::import_pipeline::WorkerFactory = Arc::new(move |token| {
            let write_transport = transport.clone();
            let ingest_transport = transport.clone();
            let scan_transport = transport.clone();
            let collector = collector.clone();
            Ok(Box::new(local::job_worker::NewRegionJobBaseWorker(
                token,
                Arc::new(move |token, job| {
                    let data = job.ingest_data()?;
                    let result = write_transport.Write(token, job, data.as_ref())?;
                    if !result.empty_job {
                        collector.Processed(result.total_bytes, result.count);
                    }
                    Ok(result)
                }),
                Arc::new(move |token, job| ingest_transport.Ingest(token, job)),
                Arc::new(|token, _| token.check()),
                Arc::new(move |token, job| {
                    let regions = scan_transport.Scan(token, &job.key_range)?;
                    Ok(local::region_job::newRegionJobs(
                        &regions,
                        &[],
                        &[job.key_range.clone()],
                        job.region_split_size,
                        job.region_split_keys,
                    )
                    .into_iter()
                    .map(|mut next| {
                        next.timestamp = job.timestamp;
                        next
                    })
                    .collect())
                }),
            )) as Box<dyn RegionJobWorker>)
        });
        let (bytes, count) = local::import_pipeline::do_import(
            &self.token,
            engine.clone(),
            self.concurrency,
            generate,
            workers,
            Default::default(),
        )?;
        if (bytes, count) != engine.ImportedStatistics() || count != engine.GetTotalLoadedKVsCount()
        {
            anyhow::bail!("external ingest statistics differ from loaded data");
        }
        Ok(())
    }
    fn GetExternalEngineConflictInfo(&self, id: i64) -> api::ConflictInfo {
        self.engines
            .lock()
            .unwrap()
            .get(&id)
            .map(|engine| engine.ConflictInfo())
            .unwrap_or_default()
    }
    fn CleanupEngine(&self, id: i64) -> anyhow::Result<()> {
        if let Some(engine) = self.engines.lock().unwrap().remove(&id) {
            let mut engine = Arc::try_unwrap(engine)
                .map_err(|_| anyhow::anyhow!("external engine {id} is still in use"))?;
            engine
                .Close()
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        }
        Ok(())
    }
    fn Close(&self) {
        self.token.cancel();
        let ids: Vec<_> = self.engines.lock().unwrap().keys().copied().collect();
        for id in ids {
            let _ = self.CleanupEngine(id);
        }
        self.transport.Close();
    }
}

/// Preserve the task's bound store and all other step constructors. The unified
/// registered executor dispatches WriteAndIngest through this native backend.
pub struct GlobalSortImportHost {
    pub inner: Arc<dyn ImportStepHost>,
    pub store: astersql_objstore_storeapi::StorageRef,
    pub transport: Arc<dyn RegionImportTransport>,
}
impl ImportStepHost for GlobalSortImportHost {
    fn TaskStore(&self) -> Arc<dyn astersql_kv::Storage + Send + Sync> {
        self.inner.TaskStore()
    }
    fn NewWriteIngestBackend(
        &self,
        task: &astersql_dxf_framework_proto::Task,
        _: &crate::TaskMeta,
        _: Arc<dyn astersql_kv::Storage + Send + Sync>,
    ) -> Result<Arc<dyn WriteIngestBackend>, astersql_errors::SharedError> {
        Ok(Arc::new(GlobalSortWriteIngestBackend::new(
            self.store.clone(),
            self.transport.clone(),
            task.GetRuntimeSlots() as usize,
        )))
    }
    fn NewStepExecutor(
        &self,
        step: astersql_dxf_framework_proto::Step,
        task: &astersql_dxf_framework_proto::Task,
        meta: &crate::TaskMeta,
        store: Arc<dyn astersql_kv::Storage + Send + Sync>,
    ) -> Result<Box<dyn execute::StepExecutor>, astersql_errors::SharedError> {
        self.inner.NewStepExecutor(step, task, meta, store)
    }
}

pub fn RegisterGlobalSortImportExecutor(
    runtime: Arc<crate::ConfiguredEncodeSortRuntime>,
    inner: Arc<dyn ImportStepHost>,
    transport: Arc<dyn RegionImportTransport>,
    retry_policy: Arc<dyn astersql_dxf_framework_taskexecutor::Extension>,
) {
    let host = Arc::new(GlobalSortImportHost {
        inner,
        store: runtime.ObjectStore.clone(),
        transport,
    });
    crate::task_executor::RegisterImportExecutor(runtime, host, retry_policy);
}

#[cfg(test)]
#[path = "write_ingest_backend_test.rs"]
mod tests;
