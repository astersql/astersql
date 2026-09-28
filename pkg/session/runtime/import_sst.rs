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
    stats: Arc<Mutex<kv::SSTImportStats>>,
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
    fn WriteAndIngest(
        &self,
        token: &local::CancellationToken,
        engine: &local::engine::Engine,
        ranges: &[local::KeyRange],
    ) -> local::Result<(i64, i64)> {
        token.check()?;
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
                store.ImportSST(engine.engine_meta.ts.load(Ordering::Acquire), pairs)
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
            stats: self.stats.clone(),
        }))
    }
    fn Close(&self) {}
}

pub(super) struct Backend {
    local: local::local::Backend,
    token: local::CancellationToken,
    seen: Arc<Mutex<HashMap<Uuid, BTreeSet<Vec<u8>>>>>,
    pub stats: Arc<Mutex<kv::SSTImportStats>>,
    directory: std::path::PathBuf,
}
impl Backend {
    pub fn new(domain: Arc<Domain>, task_id: i64) -> SessionResult<Arc<Self>> {
        let directory = std::env::temp_dir().join(format!(
            "astersql-import-{}-{task_id}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let stats = Arc::new(Mutex::new(kv::SSTImportStats::default()));
        let bridge = Arc::new(StoreBridge {
            domain,
            stats: stats.clone(),
        });
        let local = local::local::NewBackend(
            local::local::BackendConfig {
                local_store_dir: directory.to_string_lossy().into_owned(),
                duplicate_detection: true,
                ..Default::default()
            },
            bridge.clone(),
            Some(bridge),
            None,
        )
        .map_err(|error| SessionError::new(error.to_string()))?;
        Ok(Arc::new(Self {
            local,
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
