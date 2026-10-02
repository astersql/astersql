// Copyright 2026 AsterSQL.

//! Canonical TableImporterRuntime and Lightning Backend adapters. The importer
//! owns parsing/encoding/delivery; ingestctrl owns sorting and engine lifecycle;
//! the selected KV store owns the physical Write/MultiIngest transport.
use super::*;
use astersql_executor_importer as importer;
use astersql_ingestor_ingestctrl as local;
use astersql_lightning_backend as backend;
use astersql_lightning_backend_encode as encode;
use astersql_lightning_backend_kv as encoding;
use astersql_lightning_mydump as dump;
use dump::Parser;
use std::sync::atomic::Ordering;
use uuid::Uuid;

#[derive(Default)]
pub(super) struct Progress {
    pub rows: std::sync::atomic::AtomicI64,
    read: std::sync::atomic::AtomicI64,
    processed: std::sync::atomic::AtomicI64,
}
impl astersql_dxf_framework_taskexecutor_execute::Collector for Progress {
    fn Accepted(&self, bytes: i64) {
        self.read.fetch_add(bytes, Ordering::Relaxed);
    }
    fn Processed(&self, bytes: i64, rows: i64) {
        self.processed.fetch_add(bytes, Ordering::Relaxed);
        self.rows.fetch_add(rows, Ordering::Relaxed);
    }
}

fn local_error(error: impl std::fmt::Display) -> local::Error {
    local::Error::InvalidArgument(error.to_string())
}
fn backend_error(error: impl std::fmt::Display) -> backend::BackendError {
    backend::BackendError::new(error.to_string())
}

pub(super) struct Runtime {
    pub table: astersql_meta_model::TableInfo,
    pub storage: Arc<dyn dump::Storage>,
    pub config: dump::DataDivideConfig,
    pub skip_rows: u64,
    pub flags: astersql_types::Flags,
}
impl importer::TableImporterRuntime for Runtime {
    fn DataSourceType(&self) -> importer::DataSourceType {
        importer::DataSourceTypeFile
    }
    fn TableInfo(&self) -> &astersql_meta_model::TableInfo {
        &self.table
    }
    fn GetKeySpace(&self) -> Vec<u8> {
        Vec::new()
    }
    fn GetKVEncoder(
        &self,
        chunk: &dyn importer::ImportChunk,
    ) -> Result<importer::TableKVEncoder, String> {
        let config = encode::EncodingConfig {
            Table: Some(Arc::new(importer::NewTableDefinitionFromMeta(&self.table)?)),
            Path: chunk.Path().into(),
            SessionOptions: encode::SessionOptions {
                Timestamp: chunk.Timestamp(),
                AutoRandomSeed: chunk.PrevRowIDMax(),
                ..Default::default()
            },
            ..Default::default()
        };
        importer::NewTableKVEncoderFromMeta(
            &config,
            &self.table,
            Arc::new(importer::CanonicalImportDatumConverter(self.flags)),
        )
    }
    fn GetParser(
        &self,
        context: &encode::Context,
        chunk: &dyn importer::ImportChunk,
    ) -> Result<Box<dyn dump::Parser + Send>, String> {
        if context.cancelled {
            return Err("import cancelled".into());
        }
        let file = dump::SourceFileMeta {
            path: chunk.Path().into(),
            file_size: chunk.FileSize(),
            source_type: chunk.SourceType(),
            compression: chunk.Compression(),
            ..Default::default()
        };
        let mut parser = dump::openCSVParser(&file, &self.config, self.storage.as_ref())
            .map_err(|error| error.to_string())?;
        parser
            .SetPos(chunk.Offset(), chunk.PrevRowIDMax())
            .map_err(|error| error.to_string())?;
        if chunk.Offset() == 0 {
            for _ in 0..self.skip_rows {
                match parser.ReadRow() {
                    Ok(()) => {}
                    Err(dump::Error::Eof) => break,
                    Err(error) => return Err(error.to_string()),
                }
            }
            parser.SetRowID(chunk.PrevRowIDMax());
        }
        Ok(Box::new(parser))
    }
    fn TakeQueryChunks(&self) -> Result<importer::SharedQueryChunkReceiver, String> {
        Err("file importer has no SELECT source".into())
    }
}

struct StoreBridge {
    domain: Arc<Domain>,
    options: Arc<Mutex<kv::SSTImportOptions>>,
    stats: Arc<Mutex<kv::SSTImportStats>>,
    key_prefix: Vec<u8>,
}
struct ImportMonitorStop<'a>(&'a std::sync::atomic::AtomicBool);
impl Drop for ImportMonitorStop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
impl local::engine_mgr::StoreHelper for StoreBridge {
    fn GetTS(&self, token: &local::CancellationToken) -> local::Result<(i64, i64)> {
        token.check()?;
        let ts = self
            .domain
            .storage()
            .with_storage(|store| store.CurrentVersion(kv::GlobalTxnScope))
            .map_err(local_error)?
            .Ver;
        Ok(((ts >> 18) as i64, (ts & ((1 << 18) - 1)) as i64))
    }
    fn GetTiKVCodec(&self) -> String {
        "v1".into()
    }
}
impl local::local::ImportClient for StoreBridge {
    fn WriteAndIngestData(
        &self,
        token: &local::CancellationToken,
        data: &dyn local::local::engineapi::IngestData,
        ranges: &[local::KeyRange],
    ) -> local::Result<(i64, i64)> {
        token.check()?;
        let options = self
            .options
            .lock()
            .map_err(|_| local::Error::Poisoned)?
            .clone();
        let done = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let context = local::local::engineapi::Context::background();
            let monitoring_context = context.clone();
            let stopping = &done;
            let physical_context = options.context.clone();
            let monitor = scope.spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    if token.is_cancelled() {
                        monitoring_context.cancel();
                        physical_context.cancel();
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            });
            let _stop = ImportMonitorStop(&done);
            let result = (|| {
                let mut total_bytes = 0i64;
                let mut total_count = 0i64;
                let mut pool = local::local::membuf::NewPool(Vec::new());
                for range in ranges {
                    let mut iterator = data.NewIter(
                        &context,
                        &range.start,
                        &range.end,
                        Arc::get_mut(&mut pool)
                            .ok_or_else(|| local_error("shared import iterator pool"))?,
                    );
                    let encoded = (|| {
                        let mut pairs = Vec::new();
                        let mut valid = iterator.First();
                        while valid {
                            token.check()?;
                            let key = iterator
                                .Key()
                                .strip_prefix(self.key_prefix.as_slice())
                                .ok_or_else(|| {
                                    local_error("cloud index key belongs to another keyspace")
                                })?
                                .to_vec();
                            let value = iterator.Value().to_vec();
                            total_bytes += (iterator.Key().len() + value.len()) as i64;
                            total_count += 1;
                            pairs.push((key, value));
                            valid = iterator.Next();
                        }
                        if let Some(error) = iterator.Error() {
                            return Err(local_error(error));
                        }
                        Ok(pairs)
                    })();
                    let close = iterator.Close().map_err(local_error);
                    let pairs = encoded?;
                    close?;
                    if pairs.is_empty() {
                        continue;
                    }
                    let stats = self.domain.storage().with_storage(|store| {
                        let snapshot = store.GetSnapshot(kv::MaxVersion);
                        let keys = pairs
                            .iter()
                            .map(|(key, _)| kv::Key(key.clone()))
                            .collect::<Vec<_>>();
                        let existing = snapshot
                            .BatchGet(&options.context, &keys, &[])
                            .map_err(local_error)?;
                        for (key, value) in &pairs {
                            if let Some(old) = existing.get(&astersql_kv::KeyMapName(key)) {
                                if old.Value != *value {
                                    return Err(local::Error::Conflict {
                                        key: key.clone(),
                                        value: value.clone(),
                                    });
                                }
                            }
                        }
                        store
                            .ImportSSTWithOptions(data.GetTS(), pairs, options.clone())
                            .map_err(local_error)
                    })?;
                    let mut total = self.stats.lock().map_err(|_| local::Error::Poisoned)?;
                    total.keys += stats.keys;
                    total.bytes += stats.bytes;
                    total.write_rpcs += stats.write_rpcs;
                    total.ingest_rpcs += stats.ingest_rpcs;
                }
                token.check()?;
                Ok((total_bytes, total_count))
            })();
            done.store(true, Ordering::Release);
            monitor.join().unwrap();
            result
        })
    }
    fn WriteAndIngest(
        &self,
        token: &local::CancellationToken,
        engine: &local::engine::Engine,
        ranges: &[local::KeyRange],
    ) -> local::Result<(i64, i64)> {
        token.check()?;
        let options = self
            .options
            .lock()
            .map_err(|_| local::Error::Poisoned)?
            .clone();
        let pairs = engine
            .snapshot()?
            .into_iter()
            .filter(|pair| {
                ranges.iter().any(|range| {
                    pair.key >= range.start && (range.end.is_empty() || pair.key < range.end)
                })
            })
            .map(|pair| (pair.key, pair.value))
            .collect::<Vec<_>>();
        let stats = self
            .domain
            .storage()
            .with_storage(|store| {
                let snapshot = store.GetSnapshot(kv::MaxVersion);
                let keys = pairs
                    .iter()
                    .map(|(key, _)| kv::Key(key.clone()))
                    .collect::<Vec<_>>();
                if !snapshot
                    .BatchGet(&kv::Context::todo(), &keys, &[])?
                    .is_empty()
                {
                    return Err(kv::errors::New("duplicate key during physical import"));
                }
                store.ImportSSTWithOptions(
                    engine.engine_meta.ts.load(Ordering::Acquire),
                    pairs,
                    options.clone(),
                )
            })
            .map_err(local_error)?;
        let mut total = self.stats.lock().map_err(|_| local::Error::Poisoned)?;
        total.keys += stats.keys;
        total.bytes += stats.bytes;
        total.write_rpcs += stats.write_rpcs;
        total.ingest_rpcs += stats.ingest_rpcs;
        Ok((stats.bytes as i64, stats.keys as i64))
    }
    fn Close(&self) {}
}
impl local::local::ImportClientFactory for StoreBridge {
    fn Create(
        &self,
        token: &local::CancellationToken,
        _: u64,
    ) -> local::Result<Arc<dyn local::local::ImportClient>> {
        token.check()?;
        Ok(Arc::new(Self {
            domain: self.domain.clone(),
            options: self.options.clone(),
            stats: self.stats.clone(),
            key_prefix: self.key_prefix.clone(),
        }))
    }
    fn Close(&self) {}
}

pub(super) struct Backend {
    local: local::local::Backend,
    options: Arc<Mutex<kv::SSTImportOptions>>,
    token: local::CancellationToken,
    seen: Arc<Mutex<HashMap<Uuid, BTreeSet<Vec<u8>>>>>,
    pub stats: Arc<Mutex<kv::SSTImportStats>>,
    directory: std::path::PathBuf,
}
struct CloudEngine(Arc<astersql_ingestor_globalsort::engine::ExternalEngineAdapter>);
struct CloudPool(Arc<dyn local::import_pipeline::ImportPoolTuner>);
impl astersql_ingestor_globalsort::engine::WorkerPoolTuner for CloudPool {
    fn Tune(&self, concurrency: usize) {
        self.0
            .Tune(concurrency)
            .expect("failed to tune native region-job pool");
    }
}
impl local::engine_mgr::ExternalEngine for CloudEngine {
    fn LoadIngestData(
        &self,
        context: &local::local::engineapi::Context,
        out: &std::sync::mpsc::SyncSender<local::local::engineapi::DataAndRanges>,
    ) -> Result<(), local::local::engineapi::EngineError> {
        local::local::engineapi::Engine::LoadIngestData(self.0.as_ref(), context, out).map_err(
            |failure| match failure.downcast::<astersql_ingestor_globalsort::Error>() {
                Ok(failure) => match *failure {
                    astersql_ingestor_globalsort::Error::DuplicateKey { key, value } => {
                        Box::new(local::Error::Conflict { key, value })
                            as local::local::engineapi::EngineError
                    }
                    astersql_ingestor_globalsort::Error::Cancelled => {
                        Box::new(local::Error::Cancelled)
                    }
                    astersql_ingestor_globalsort::Error::Closed => Box::new(local::Error::Closed),
                    failure => Box::new(failure),
                },
                Err(failure) => failure,
            },
        )
    }
    fn SetWorkerPool(&self, pool: Arc<dyn local::import_pipeline::ImportPoolTuner>) {
        self.0
            .ResourceHandle()
            .SetWorkerPool(Arc::new(CloudPool(pool)));
    }
    fn GetTotalLoadedKVsCount(&self) -> i64 {
        self.0.GetTotalLoadedKVsCount()
    }
    fn ID(&self) -> String {
        local::local::engineapi::Engine::ID(self.0.as_ref())
    }
    fn KVStatistics(&self) -> (i64, i64) {
        local::local::engineapi::Engine::KVStatistics(self.0.as_ref())
    }
    fn ImportedStatistics(&self) -> (i64, i64) {
        local::local::engineapi::Engine::ImportedStatistics(self.0.as_ref())
    }
    fn ConflictInfo(&self) -> local::ConflictInfo {
        let info = local::local::engineapi::Engine::ConflictInfo(self.0.as_ref());
        local::ConflictInfo {
            count: info.Count,
            size: self.0.RecordedDuplicateSize() as u64,
        }
    }
    fn ConflictFiles(&self) -> Vec<String> {
        local::local::engineapi::Engine::ConflictInfo(self.0.as_ref()).Files
    }
    fn GetKeyRange(&self) -> local::Result<local::KeyRange> {
        let (start, end) =
            local::local::engineapi::Engine::GetKeyRange(self.0.as_ref()).map_err(local_error)?;
        Ok(local::KeyRange { start, end })
    }
    fn GetRegionSplitKeys(&self) -> local::Result<Vec<Vec<u8>>> {
        local::local::engineapi::Engine::GetRegionSplitKeys(self.0.as_ref()).map_err(local_error)
    }
    fn Close(&self) -> local::Result<()> {
        self.0.CloseShared().map_err(local_error)
    }
}
impl Backend {
    pub fn disk_quota_pressure(
        &self,
        quota: i64,
    ) -> astersql_ingestor_ingestctrl::disk_quota::DiskQuotaResult {
        astersql_ingestor_ingestctrl::disk_quota::CheckDiskQuota(&self.local, quota)
    }
    pub(super) fn register_external(
        &self,
        id: Uuid,
        engine: astersql_ingestor_globalsort::engine::Engine,
        token: astersql_ingestor_globalsort::reader::CancellationToken,
    ) -> SessionResult<Arc<astersql_ingestor_globalsort::engine::ExternalEngineAdapter>> {
        let adapter = Arc::new(
            astersql_ingestor_globalsort::engine::ExternalEngineAdapter::new(engine, token),
        );
        self.local
            .RegisterExternalEngine(
                local::EngineId(id.as_u128()),
                Arc::new(CloudEngine(adapter.clone())),
            )
            .map_err(|error| SessionError::new(error.to_string()))?;
        Ok(adapter)
    }
    pub(super) fn import_external(
        &self,
        token: &local::CancellationToken,
        id: Uuid,
    ) -> SessionResult<()> {
        self.import_external_native(token, id)
            .map_err(|error| SessionError::new(error.to_string()))
    }
    pub(super) fn import_external_native(
        &self,
        token: &local::CancellationToken,
        id: Uuid,
    ) -> local::Result<()> {
        self.local
            .ImportEngine(token, local::EngineId(id.as_u128()), 0)
    }
    pub(super) fn set_import_options(&self, options: kv::SSTImportOptions) -> SessionResult<()> {
        *self
            .options
            .lock()
            .map_err(|_| SessionError::new("import options poisoned"))? = options;
        Ok(())
    }
    pub(super) fn set_worker_concurrency(&self, concurrency: usize) {
        self.local.SetWorkerConcurrency(concurrency);
    }
    pub(super) fn cleanup_external(&self, id: Uuid) -> SessionResult<()> {
        self.local
            .CleanupEngine(local::EngineId(id.as_u128()))
            .map_err(|error| SessionError::new(error.to_string()))
    }
    pub fn new(domain: Arc<Domain>, task_id: i64) -> SessionResult<Arc<Self>> {
        Self::new_with_options(domain, task_id, kv::SSTImportOptions::default())
    }
    pub(super) fn new_with_options(
        domain: Arc<Domain>,
        task_id: i64,
        options: kv::SSTImportOptions,
    ) -> SessionResult<Arc<Self>> {
        Self::new_with_key_prefix(
            domain,
            task_id,
            options,
            Vec::new(),
            local::local::BackendConfig::default().worker_concurrency,
        )
    }
    pub(super) fn new_with_key_prefix(
        domain: Arc<Domain>,
        task_id: i64,
        options: kv::SSTImportOptions,
        key_prefix: Vec<u8>,
        concurrency: usize,
    ) -> SessionResult<Arc<Self>> {
        let directory = std::env::temp_dir().join(format!(
            "astersql-import-{}-{task_id}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let stats = Arc::new(Mutex::new(kv::SSTImportStats::default()));
        let options = Arc::new(Mutex::new(options));
        let bridge = Arc::new(StoreBridge {
            domain,
            options: options.clone(),
            stats: stats.clone(),
            key_prefix,
        });
        let local = local::local::NewBackend(
            local::local::BackendConfig {
                local_store_dir: directory.to_string_lossy().into_owned(),
                duplicate_detection: true,
                worker_concurrency: concurrency.max(1),
                ..Default::default()
            },
            bridge.clone(),
            Some(bridge),
            None,
        )
        .map_err(|error| SessionError::new(error.to_string()))?;
        Ok(Arc::new(Self {
            local,
            options,
            stats,
            directory,
            token: local::CancellationToken::default(),
            seen: Arc::new(Mutex::new(HashMap::new())),
        }))
    }
}

/// Reuse the concrete SST local backend for IMPORT INTO engine lifecycle tests
/// and host wiring; each task owns its temporary directory and cleanup.
pub fn NewImportLocalBackend(
    domain: Arc<Domain>,
    task_id: i64,
) -> SessionResult<Arc<dyn backend::Backend>> {
    Backend::new(domain, task_id).map(|backend| backend as Arc<dyn backend::Backend>)
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.local.CleanupAllLocalEngines();
        self.local.Close();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
impl backend::Backend for Backend {
    fn Close(&self) {
        self.local.Close();
    }
    fn RetryImportDelay(&self) -> std::time::Duration {
        self.local.RetryImportDelay()
    }
    fn ShouldPostProcess(&self) -> bool {
        self.local.ShouldPostProcess()
    }
    fn OpenEngine(
        &self,
        _: &encode::Context,
        _: &backend::EngineConfig,
        id: Uuid,
    ) -> Result<(), backend::BackendError> {
        self.local
            .OpenEngine(&self.token, local::EngineId(id.as_u128()))
            .map_err(backend_error)?;
        self.seen.lock().unwrap().insert(id, BTreeSet::new());
        Ok(())
    }
    fn CloseEngine(
        &self,
        _: &encode::Context,
        _: Option<&backend::EngineConfig>,
        id: Uuid,
    ) -> Result<(), backend::BackendError> {
        self.local
            .CloseEngine(local::EngineId(id.as_u128()))
            .map_err(backend_error)
    }
    fn ImportEngine(
        &self,
        _: &encode::Context,
        id: Uuid,
        _: i64,
        _: i64,
    ) -> Result<(), backend::BackendError> {
        self.local
            .ImportEngine(&self.token, local::EngineId(id.as_u128()), 0)
            .map_err(backend_error)
    }
    fn CleanupEngine(&self, _: &encode::Context, id: Uuid) -> Result<(), backend::BackendError> {
        self.local
            .CleanupEngine(local::EngineId(id.as_u128()))
            .map_err(backend_error)?;
        self.seen.lock().unwrap().remove(&id);
        Ok(())
    }
    fn FlushEngine(&self, _: &encode::Context, id: Uuid) -> Result<(), backend::BackendError> {
        self.local
            .FlushEngine(local::EngineId(id.as_u128()))
            .map_err(backend_error)
    }
    fn FlushAllEngines(&self, _: &encode::Context) -> Result<(), backend::BackendError> {
        self.local.FlushAllEngines().map_err(backend_error)
    }
    fn LocalWriter(
        &self,
        _: &encode::Context,
        _: &backend::LocalWriterConfig,
        id: Uuid,
    ) -> Result<Box<dyn backend::EngineWriter>, backend::BackendError> {
        Ok(Box::new(Writer {
            writer: self
                .local
                .LocalWriter(local::EngineId(id.as_u128()), 1024)
                .map_err(backend_error)?,
            seen: self.seen.clone(),
            id,
            closed: false,
        }))
    }
}
struct Writer {
    writer: local::engine::Writer,
    seen: Arc<Mutex<HashMap<Uuid, BTreeSet<Vec<u8>>>>>,
    id: Uuid,
    closed: bool,
}
impl backend::EngineWriter for Writer {
    fn AppendRows(
        &mut self,
        context: &encode::Context,
        _: &[String],
        rows: &dyn encode::Rows,
    ) -> Result<(), backend::BackendError> {
        if context.cancelled {
            return Err(backend_error("import cancelled"));
        }
        let groups: Vec<_> = if let Some(rows) = rows.as_any().downcast_ref::<encoding::Pairs>() {
            rows.Pairs.iter().collect()
        } else if let Some(rows) = rows.as_any().downcast_ref::<encoding::GroupedPairs>() {
            rows.0.values().flatten().collect()
        } else {
            return Err(backend_error("SST engine requires encoded KV rows"));
        };
        let mut seen = self.seen.lock().unwrap();
        let seen = seen
            .get_mut(&self.id)
            .ok_or_else(|| backend_error("engine is closed"))?;
        for pair in groups {
            if !seen.insert(pair.key.clone()) {
                return Err(backend_error("duplicate key in import engine"));
            }
            self.writer
                .Append(pair.key.clone(), pair.val.clone())
                .map_err(backend_error)?;
        }
        Ok(())
    }
    fn IsSynced(&self) -> bool {
        self.closed
    }
    fn Close(
        &mut self,
        _: &encode::Context,
    ) -> Result<Option<backend::ChunkFlushStatus>, backend::BackendError> {
        self.writer.Close().map_err(backend_error)?;
        self.closed = true;
        Ok(Some(backend::ChunkFlushStatus { flushed: true }))
    }
}
