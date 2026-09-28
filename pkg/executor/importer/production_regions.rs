// Copyright 2026 AsterSQL.

use crate::{
    Chunk, DiskQuotaState, ImportRuntimeConfig, LoadDataController, Plan, RemoteChecksum,
    RemoteChecksumError, TableImporterService, TableRegion,
};
use astersql_lightning_backend::{Backend, ClosedEngine};
use astersql_lightning_backend_encode::Table as EncodingTable;
use astersql_lightning_backend_kv::AllocatorType;
use astersql_lightning_mydump as mydump;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) struct LocalRegionStorage {
    pub(crate) parent: PathBuf,
}

impl mydump::Storage for LocalRegionStorage {
    fn open(
        &self,
        path: &str,
        compression: mydump::Compression,
    ) -> Result<Box<dyn Read + Send>, mydump::MydumpError> {
        if compression != mydump::Compression::None {
            return Err(mydump::MydumpError::Io(
                "compressed local region reads are not used by CSV splitting".into(),
            ));
        }
        let file = std::fs::File::open(self.parent.join(path))?;
        Ok(Box::new(file))
    }
}

/// Go mydump region splitting for server-disk files; host handles the remaining runtime methods.
pub struct HostTableImporterService {
    pub Host: Arc<dyn TableImporterService>,
}

impl TableImporterService for HostTableImporterService {
    fn RuntimeConfig(&self) -> ImportRuntimeConfig {
        self.Host.RuntimeConfig()
    }
    fn NewEncodingTable(
        &self,
        controller: &LoadDataController,
    ) -> Result<Arc<dyn EncodingTable>, String> {
        self.Host.NewEncodingTable(controller)
    }
    fn NewBackend(
        &self,
        controller: &LoadDataController,
        sort_directory: &Path,
    ) -> Result<Arc<dyn Backend>, String> {
        self.Host.NewBackend(controller, sort_directory)
    }
    fn RegionSplitSizeKeys(&self) -> Result<(i64, i64), String> {
        self.Host.RegionSplitSizeKeys()
    }
    fn NewParser(
        &self,
        controller: &LoadDataController,
        chunk: &Chunk,
    ) -> Result<Box<dyn mydump::Parser + Send>, String> {
        self.Host.NewParser(controller, chunk)
    }
    fn EstimateParquetReaderMemory(
        &self,
        controller: &LoadDataController,
        path: &str,
    ) -> Result<i64, String> {
        self.Host.EstimateParquetReaderMemory(controller, path)
    }
    fn MakeTableRegions(
        &self,
        controller: &LoadDataController,
        adjusted_engine_size: i64,
    ) -> Result<Vec<TableRegion>, String> {
        let source = Path::new(&controller.Plan.Path);
        if !source.is_absolute() {
            return self.Host.MakeTableRegions(controller, adjusted_engine_size);
        }
        let table_info = controller.Table.Meta();
        if table_info.Columns.is_empty() {
            return Err("import target table has no columns".into());
        }
        let data_files = controller
            .DataFiles()
            .iter()
            .map(|file| mydump::FileInfo {
                file_meta: mydump::FileMeta {
                    path: file.path.clone(),
                    file_size: file.file_size,
                    real_size: file.real_size,
                    source_type: file.source_type,
                    compression: file.compression,
                    sort_key: file.sort_key.clone(),
                },
                extend_data: file.extend_data.clone(),
            })
            .collect();
        let table = mydump::MDTableMeta {
            db: controller.Plan.DBName.clone(),
            name: table_info.Name.O.clone(),
            data_files,
            ..Default::default()
        };
        let mut config = mydump::NewDataDivideConfig();
        config.column_count = table_info.Columns.len();
        config.engine_data_size = adjusted_engine_size as f64;
        config.engine_concurrency = controller.Plan.ThreadCnt;
        config.strict_format = controller.Plan.SplitFile;
        config.csv = controller.GenerateCSVConfig();
        config.charset = controller
            .Plan
            .Charset
            .clone()
            .unwrap_or_else(|| "utf8mb4".into());
        let parent = source
            .parent()
            .ok_or_else(|| "server-disk path has no parent".to_owned())?;
        let storage = LocalRegionStorage {
            parent: parent.to_path_buf(),
        };
        let regions = mydump::MakeTableRegions(&table, &config, &storage)
            .map_err(|error| error.to_string())?;
        Ok(regions
            .into_iter()
            .map(|region| TableRegion {
                EngineID: region.engine_id,
                File: region.file_meta,
                Offset: region.chunk.offset,
                EndOffset: region.chunk.end_offset,
                PrevRowIDMax: region.chunk.prev_row_id_max,
                RowIDMax: region.chunk.row_id_max,
            })
            .collect())
    }
    fn EstimateCompactionThreshold(&self, raw_index_bytes: i64) -> i64 {
        self.Host.EstimateCompactionThreshold(raw_index_bytes)
    }
    fn ImportedKVCount(&self, engine: &ClosedEngine) -> i64 {
        self.Host.ImportedKVCount(engine)
    }
    fn DiskCapacity(&self, sort_directory: &Path) -> Result<u64, String> {
        self.Host.DiskCapacity(sort_directory)
    }
    fn CheckDiskQuota(&self, backend: &dyn Backend, quota: i64) -> DiskQuotaState {
        self.Host.CheckDiskQuota(backend, quota)
    }
    fn FlushAndImportLargeEngines(
        &self,
        backend: &dyn Backend,
        engine_ids: &[i32],
    ) -> Result<(), String> {
        self.Host.FlushAndImportLargeEngines(backend, engine_ids)
    }
    fn RebaseAllocatorBases(
        &self,
        maximum_ids: &HashMap<AllocatorType, i64>,
        plan: &Plan,
    ) -> Result<(), String> {
        self.Host.RebaseAllocatorBases(maximum_ids, plan)
    }
    fn RemoteChecksumTableBySQL(
        &self,
        plan: &Plan,
        concurrency: usize,
        backoff_weight: i32,
    ) -> Result<RemoteChecksum, RemoteChecksumError> {
        self.Host
            .RemoteChecksumTableBySQL(plan, concurrency, backoff_weight)
    }
    fn FlushTableStats(&self, table_id: i64, imported_rows: i64) -> Result<(), String> {
        self.Host.FlushTableStats(table_id, imported_rows)
    }
    fn AllocatorMaximums(&self) -> HashMap<AllocatorType, i64> {
        self.Host.AllocatorMaximums()
    }
}
