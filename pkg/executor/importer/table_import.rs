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

// 单表 IMPORT INTO 的编排与后处理。
//
// 负责准备本地排序目录、打开 Lightning 数据/索引引擎、按 chunk 导入、
// 磁盘配额控制、选行导入（IMPORT FROM SELECT），以及校验和（checksum）与
// 自增分配器 rebase 等收尾步骤。Region（数据分片）分裂参数用于控制导入时的切分粒度。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use astersql_lightning_backend::{
    Backend, ClosedEngine, EngineConfig, EngineManager, LocalEngineConfig, OpenedEngine,
};
use astersql_lightning_backend_encode::{
    Context, EncodingConfig, SessionOptions, Table as EncodingTable,
};
use astersql_lightning_backend_kv::AllocatorType;
use astersql_lightning_mydump::{Compression, Parser, SourceFileMeta, SourceType};
use astersql_lightning_verification::{KVChecksum, KVGroupChecksum};
use astersql_meta_model::TableInfo;

use crate::{
    DataSourceType, ImportChunk, LoadDataController, NewTableKVEncoder,
    NewTableKVEncoderForDupResolve, Plan, PostOpLevel, QueryChunk, SharedQueryChunkReceiver,
    TableImporterRuntime, TableKVEncoder,
};

/// 周期性检查本地排序目录磁盘配额的默认间隔。
pub static CheckDiskQuotaInterval: Duration = Duration::from_secs(10);
/// 单个引擎默认最大数据体量（字节）。
pub static defaultMaxEngineSize: i64 = 5 * 96 * 1024 * 1024;
/// 索引引擎的固定 engine ID（与数据引擎正数 ID 区分）。
pub const IndexEngineID: i32 = -1;

#[derive(Clone, Debug, Default)]
/// 导入任务中的一个文件片段：路径、偏移区间、行号范围与压缩类型。
pub struct Chunk {
    pub Path: String,
    pub FileSize: i64,
    pub Offset: i64,
    pub EndOffset: i64,
    pub PrevRowIDMax: i64,
    pub RowIDMax: i64,
    pub Type: SourceType,
    pub Compression: Compression,
    pub Timestamp: i64,
}

impl Chunk {
    /// 以 `路径:偏移` 作为 chunk 唯一键。
    pub fn GetKey(&self) -> String {
        format!("{}:{}", self.Path, self.Offset)
    }

    /// Original file bytes in this chunk; Parquet offsets represent rows.
    pub fn GetSize(&self) -> i64 {
        ImportChunk::GetSize(self)
    }

    /// 转换为 mydump 使用的源文件元信息。
    pub fn toSourceFileMeta(&self) -> SourceFileMeta {
        SourceFileMeta {
            path: self.Path.clone(),
            file_size: self.FileSize,
            source_type: self.Type,
            compression: self.Compression,
            ..SourceFileMeta::default()
        }
    }
}

/// 将 Chunk 适配为 chunk 处理管线所需的 ImportChunk 接口。
impl ImportChunk for Chunk {
    fn Key(&self) -> String {
        self.GetKey()
    }
    fn Path(&self) -> &str {
        &self.Path
    }
    fn FileSize(&self) -> i64 {
        self.FileSize
    }
    fn Offset(&self) -> i64 {
        self.Offset
    }
    fn EndOffset(&self) -> i64 {
        self.EndOffset
    }
    fn PrevRowIDMax(&self) -> i64 {
        self.PrevRowIDMax
    }
    fn RowIDMax(&self) -> i64 {
        self.RowIDMax
    }
    fn SourceType(&self) -> SourceType {
        self.Type
    }
    fn Compression(&self) -> Compression {
        self.Compression
    }
    fn Timestamp(&self) -> i64 {
        self.Timestamp
    }
}

#[derive(Clone, Debug, Default)]
/// 导入运行时环境：临时目录、端口、PD 地址、keyspace 与 Region 分裂参数。
pub struct ImportRuntimeConfig {
    pub TempDir: PathBuf,
    pub Port: u16,
    pub PDAddress: String,
    /// TiKV codec keyspace bytes; API V2 prefixes contain zero bytes.
    pub Keyspace: Vec<u8>,
    pub RegionSplitSize: i64,
    pub RegionSplitKeys: i64,
}

#[derive(Clone, Debug, Default)]
/// 按引擎划分后的表区域描述，对应一个待导入的文件偏移区间。
pub struct TableRegion {
    pub EngineID: i32,
    pub File: SourceFileMeta,
    pub Offset: i64,
    pub EndOffset: i64,
    pub PrevRowIDMax: i64,
    pub RowIDMax: i64,
}

#[derive(Clone, Debug, Default)]
/// 一次磁盘配额检查的结果：超限引擎列表与累计磁盘/内存占用。
pub struct DiskQuotaState {
    pub LargeEngineIDs: Vec<i32>,
    pub InProgressLargeEngines: usize,
    pub TotalDiskSize: i64,
    pub TotalMemorySize: i64,
}

/// 表导入对外部系统的依赖边界：后端、解析器、Region 划分、配额与远程校验和。
pub struct AllocatorRebaseBindings {
    pub Requirement: Arc<dyn astersql_lightning_common::AutoIDRequirement>,
    /// Release the host's AutoID discovery connection after etcd is closed.
    pub ResetConnection: Box<dyn FnOnce() + Send>,
}

pub trait TableImporterService: Send + Sync {
    fn RuntimeConfig(&self) -> ImportRuntimeConfig;
    fn AllocatorMetadataStore(&self) -> Option<Arc<dyn astersql_metaservice::EtcdMetadataStore>> {
        None
    }
    fn AllocatorEtcdConfig(&self) -> Result<astersql_metaservice::EtcdDialConfig, String> {
        Ok(Default::default())
    }
    fn NewAllocatorRebaseBindings(
        &self,
        _client: &astersql_metaservice::NamespacedEtcdClient,
    ) -> Result<AllocatorRebaseBindings, String> {
        Err("import host does not expose allocator discovery bindings".into())
    }

    fn NewEncodingTable(
        &self,
        controller: &LoadDataController,
    ) -> Result<Arc<dyn EncodingTable>, String>;
    fn NewBackend(
        &self,
        controller: &LoadDataController,
        sort_directory: &Path,
    ) -> Result<Arc<dyn Backend>, String>;
    fn RegionSplitSizeKeys(&self) -> Result<(i64, i64), String>;
    fn NewParser(
        &self,
        controller: &LoadDataController,
        chunk: &Chunk,
    ) -> Result<Box<dyn Parser + Send>, String>;
    /// Host parser boundary for Parquet temporal conversion. Existing hosts
    /// may read the location directly from the controller until they override
    /// this method; the selected location is explicit at the call site.
    fn NewParserWithParquetLocation(
        &self,
        controller: &LoadDataController,
        chunk: &Chunk,
        location: &str,
    ) -> Result<Box<dyn Parser + Send>, String> {
        let _ = location;
        self.NewParser(controller, chunk)
    }
    fn EstimateParquetReaderMemory(
        &self,
        controller: &LoadDataController,
        path: &str,
    ) -> Result<i64, String>;
    fn MakeTableRegions(
        &self,
        controller: &LoadDataController,
        adjusted_engine_size: i64,
    ) -> Result<Vec<TableRegion>, String>;
    fn EstimateCompactionThreshold(&self, raw_index_bytes: i64) -> i64;
    fn ImportedKVCount(&self, engine: &ClosedEngine) -> i64;
    fn DiskCapacity(&self, sort_directory: &Path) -> Result<u64, String>;
    fn CheckDiskQuota(&self, backend: &dyn Backend, quota: i64) -> DiskQuotaState;
    fn FlushAndImportLargeEngines(
        &self,
        backend: &dyn Backend,
        engine_ids: &[i32],
    ) -> Result<(), String>;
    fn RebaseAllocatorBases(
        &self,
        maximum_ids: &HashMap<AllocatorType, i64>,
        plan: &Plan,
    ) -> Result<(), String>;
    fn RemoteChecksumTableBySQL(
        &self,
        plan: &Plan,
        concurrency: usize,
        backoff_weight: i32,
    ) -> Result<RemoteChecksum, RemoteChecksumError>;
    fn FlushTableStats(&self, table_id: i64, imported_rows: i64) -> Result<(), String>;
    fn AllocatorMaximums(&self) -> HashMap<AllocatorType, i64>;
}

/// 通过服务工厂为目标表创建编码用表元信息。
pub fn newEncodingTable(
    controller: &LoadDataController,
    service: &dyn TableImporterService,
) -> Result<Arc<dyn EncodingTable>, String> {
    service.NewEncodingTable(controller)
}

/// 确保 import 根目录存在，并清理同 ID 残留的 sort 目录后返回目标路径。
pub fn prepareSortDir(
    controller: &LoadDataController,
    identifier: &str,
    configuration: &ImportRuntimeConfig,
) -> Result<PathBuf, String> {
    let _ = controller;
    prepareSortDirPath(identifier, configuration)
}

pub(crate) fn prepareSortDirPath(
    identifier: &str,
    configuration: &ImportRuntimeConfig,
) -> Result<PathBuf, String> {
    let import_directory = GetImportRootDir(configuration);
    let sort_directory = import_directory.join(identifier);
    // 根路径若是文件则删除后重建；不存在则创建；已是目录则复用。
    match std::fs::metadata(&import_directory) {
        Ok(metadata) if !metadata.is_dir() => {
            std::fs::remove_file(&import_directory).map_err(|error| error.to_string())?;
            std::fs::create_dir_all(&import_directory).map_err(|error| error.to_string())?;
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&import_directory).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    // 同一 job 的旧 sort 目录必须清空，避免脏数据混入。
    if sort_directory.exists() {
        std::fs::remove_dir_all(&sort_directory).map_err(|error| error.to_string())?;
    }
    Ok(sort_directory)
}

/// 从服务获取 PD 配置的 Region 分裂大小与 key 数。
pub fn GetRegionSplitSizeKeys(service: &dyn TableImporterService) -> Result<(i64, i64), String> {
    getRegionSplitSizeKeysWith(|| service.RegionSplitSizeKeys())
}

pub(crate) fn getRegionSplitSizeKeysWith(
    get_region_split_size_keys: impl FnOnce() -> Result<(i64, i64), String>,
) -> Result<(i64, i64), String> {
    get_region_split_size_keys()
}

/// 单表导入会话：持有控制器、后端引擎、排序目录与可选的选行 channel。
/// Tracks local engines until their import and cleanup both complete.
/// A failed subtask can release every remaining engine before the same IDs are retried.
pub struct LocalEngineCleanup {
    backend: Arc<dyn Backend>,
    table_name: String,
    opened: Mutex<HashSet<i32>>,
}

impl LocalEngineCleanup {
    pub fn new(backend: Arc<dyn Backend>, table_name: String) -> Self {
        Self {
            backend,
            table_name,
            opened: Mutex::new(HashSet::new()),
        }
    }

    pub fn Record(&self, engine_id: i32) {
        self.opened.lock().unwrap().insert(engine_id);
    }

    pub fn Forget(&self, engine_id: i32) {
        self.opened.lock().unwrap().remove(&engine_id);
    }

    pub fn CleanupAll(&self, context: &Context) {
        let engine_ids: Vec<_> = self.opened.lock().unwrap().drain().collect();
        for engine_id in engine_ids {
            let (_, uuid) =
                astersql_lightning_backend::MakeUUID(&self.table_name, i64::from(engine_id));
            if self.backend.CleanupEngine(context, uuid).is_err() {
                self.opened.lock().unwrap().insert(engine_id);
            }
        }
    }
}

pub struct TableImporter {
    pub LoadDataController: LoadDataController,
    id: String,
    backend: Arc<dyn Backend>,
    engine_manager: EngineManager,
    local_engine_cleanup: LocalEngineCleanup,
    table_info: TableInfo,
    encoding_table: Arc<dyn EncodingTable>,
    keyspace: Vec<u8>,
    region_split_size: i64,
    region_split_keys: i64,
    disk_quota: i64,
    disk_quota_lock: Arc<Mutex<()>>,
    sort_directory: PathBuf,
    chunk_receiver: Option<SharedQueryChunkReceiver>,
    service: Arc<dyn TableImporterService>,
}

/// 创建表导入器：准备 sort 目录、编码表、后端，并调整 Region 分裂与磁盘配额。
pub fn NewTableImporter(
    controller: LoadDataController,
    identifier: impl Into<String>,
    service: Arc<dyn TableImporterService>,
) -> Result<TableImporter, String> {
    let identifier = identifier.into();
    let configuration = service.RuntimeConfig();
    let sort_directory = prepareSortDir(&controller, &identifier, &configuration)?;
    let encoding_table = newEncodingTable(&controller, service.as_ref())?;
    let backend = service.NewBackend(&controller, &sort_directory)?;
    let engine_manager = astersql_lightning_backend::MakeEngineManager(Arc::clone(&backend));
    let (pd_split_size, pd_split_keys) = service.RegionSplitSizeKeys()?;
    // 取配置与 PD 参数的较大值再乘 2，降低导入后 Region 过碎。
    let region_split_size = configuration
        .RegionSplitSize
        .max(pd_split_size)
        .saturating_mul(2);
    let region_split_keys = configuration
        .RegionSplitKeys
        .max(pd_split_keys)
        .saturating_mul(2);
    let disk_quota = adjustDiskQuota(
        controller.Plan.DiskQuota.0,
        &sort_directory,
        service.as_ref(),
    );
    let table_info = controller.Table.Meta().clone();
    let local_engine_cleanup =
        LocalEngineCleanup::new(Arc::clone(&backend), controller.FullTableName());
    Ok(TableImporter {
        LoadDataController: controller,
        id: identifier,
        backend,
        engine_manager,
        local_engine_cleanup,
        table_info,
        encoding_table,
        keyspace: configuration.Keyspace,
        region_split_size,
        region_split_keys,
        disk_quota,
        disk_quota_lock: Arc::new(Mutex::new(())),
        sort_directory,
        chunk_receiver: None,
        service,
    })
}

/// 测试入口：与正式构造相同，便于注入 mock 服务。
pub fn NewTableImporterForTest(
    controller: LoadDataController,
    identifier: impl Into<String>,
    service: Arc<dyn TableImporterService>,
) -> Result<TableImporter, String> {
    NewTableImporter(controller, identifier, service)
}

impl TableImporter {
    pub fn AllocatorMaximums(&self) -> HashMap<AllocatorType, i64> {
        self.service.AllocatorMaximums()
    }

    /// 返回当前 keyspace（多租户键空间前缀）字节。
    pub fn GetKeySpace(&self) -> Vec<u8> {
        self.keyspace.clone()
    }

    /// 估算读取指定 Parquet 文件所需内存。
    pub fn EstimateParquetReaderMemory(&self, path: &str, file_size: i64) -> Result<i64, String> {
        let peak = self
            .service
            .EstimateParquetReaderMemory(&self.LoadDataController, path)?;
        if peak <= 0
            || file_size <= 0
            || file_size as u64
                > astersql_dumpformat_parquetfile::source_reader::WHOLE_FILE_THRESHOLD
        {
            return Ok(peak);
        }
        let file = SourceFileMeta {
            path: path.into(),
            file_size,
            source_type: SourceType::Parquet,
            ..Default::default()
        };
        let parser = self
            .LoadDataController
            .OpenParquetFile(&Default::default(), &file)?;
        parser
            .adjust_memory_estimate(peak)
            .map_err(|e| e.to_string())
    }

    /// 按 chunk 会话选项（SQL mode、时间戳、自增种子）构建行编码器。
    pub fn getKVEncoder(&self, chunk: &Chunk) -> Result<TableKVEncoder, String> {
        let config = EncodingConfig {
            SessionOptions: SessionOptions {
                SQLMode: self.LoadDataController.Plan.SQLMode.0 as u64,
                Timestamp: chunk.Timestamp,
                SysVars: self.LoadDataController.Plan.ImportantSysVars.clone(),
                AutoRandomSeed: chunk.PrevRowIDMax,
                ..SessionOptions::default()
            },
            Path: chunk.Path.clone(),
            Table: Some(Arc::clone(&self.encoding_table)),
            ..EncodingConfig::default()
        };
        NewTableKVEncoder(&config, &self.LoadDataController)
    }

    /// 去重解析路径使用的编码器：启用恒等 AutoRowID。
    pub fn GetKVEncoderForDupResolve(&self) -> Result<TableKVEncoder, String> {
        let config = EncodingConfig {
            SessionOptions: SessionOptions {
                SQLMode: self.LoadDataController.Plan.SQLMode.0 as u64,
                SysVars: self.LoadDataController.Plan.ImportantSysVars.clone(),
                ..SessionOptions::default()
            },
            Table: Some(Arc::clone(&self.encoding_table)),
            UseIdentityAutoRowID: true,
            ..EncodingConfig::default()
        };
        NewTableKVEncoderForDupResolve(&config, &self.LoadDataController)
    }

    /// 所有数据文件真实大小之和乘以索引数，用于估算压缩阈值。
    fn getTotalRawFileSize(&self, index_count: i64) -> i64 {
        self.LoadDataController
            .DataFiles()
            .iter()
            .fold(0_i64, |total, file| total.saturating_add(file.real_size))
            .saturating_mul(index_count)
    }

    /// 打开索引引擎；按索引数估算并配置本地 Compact 阈值。
    pub fn OpenIndexEngine(
        &self,
        context: &Context,
        engine_id: i32,
    ) -> Result<OpenedEngine, String> {
        let mut index_count = self.table_info.Indices.len() as i64;
        // 聚簇主键已计入表数据，索引计数需减一。
        if self.table_info.PKIsHandle || self.table_info.IsCommonHandle {
            index_count = index_count.saturating_sub(1);
        }
        let threshold = self
            .service
            .EstimateCompactionThreshold(self.getTotalRawFileSize(index_count));
        let config = EngineConfig {
            TableInfo: Some(astersql_lightning_backend::TableInfo {
                name: self.table_info.Name.O.clone(),
                id: self.table_info.ID,
                ..Default::default()
            }),
            Local: LocalEngineConfig {
                Compact: threshold > 0,
                CompactConcurrency: 4,
                CompactThreshold: threshold,
                BlockSize: 16 * 1024,
            },
            ..EngineConfig::default()
        };
        let opened = self
            .engine_manager
            .OpenEngine(
                context,
                &config,
                &self.LoadDataController.FullTableName(),
                engine_id,
            )
            .map_err(|error| error.to_string())?;
        self.local_engine_cleanup.Record(engine_id);
        Ok(opened)
    }

    /// 打开指定 ID 的数据引擎。
    pub fn OpenDataEngine(
        &self,
        context: &Context,
        engine_id: i32,
    ) -> Result<OpenedEngine, String> {
        let config = EngineConfig {
            TableInfo: Some(astersql_lightning_backend::TableInfo {
                name: self.table_info.Name.O.clone(),
                id: self.table_info.ID,
                ..Default::default()
            }),
            ..EngineConfig::default()
        };
        let opened = self
            .engine_manager
            .OpenEngine(
                context,
                &config,
                &self.LoadDataController.FullTableName(),
                engine_id,
            )
            .map_err(|error| error.to_string())?;
        self.local_engine_cleanup.Record(engine_id);
        Ok(opened)
    }

    /// 将关闭后的引擎导入 TiKV，再清理本地引擎文件；索引引擎不计 KV 行数。
    pub fn ImportAndCleanup(
        &self,
        context: &Context,
        closed_engine: &ClosedEngine,
    ) -> Result<i64, String> {
        let import_result = closed_engine
            .Import(context, self.region_split_size, self.region_split_keys)
            .map_err(|error| error.to_string());
        // 索引引擎导入后不参与行数统计。
        let kv_count = if closed_engine.GetID() == IndexEngineID {
            0
        } else {
            self.service.ImportedKVCount(closed_engine)
        };
        let cleanup_result = closed_engine
            .Cleanup(context)
            .map_err(|error| error.to_string());
        if cleanup_result.is_ok() {
            self.local_engine_cleanup.Forget(closed_engine.GetID());
        }
        import_result.and(cleanup_result).map(|_| kv_count)
    }

    /// Go import step 的失败收尾：清理该 importer 尚未导入并清理的全部本地引擎。
    pub fn CleanupAllLocalEngines(&self, context: &Context) {
        self.local_engine_cleanup.CleanupAll(context);
    }

    /// 返回底层 Lightning Backend。
    pub fn Backend(&self) -> &dyn Backend {
        self.backend.as_ref()
    }

    /// 关闭控制器与后端，释放资源。
    pub fn Close(&mut self) {
        self.LoadDataController.Close();
        self.backend.Close();
    }

    /// 检查磁盘配额；若有超限引擎则触发刷盘并导入。
    pub fn CheckDiskQuotaOnce(&self) -> Result<DiskQuotaState, String> {
        let state = self
            .service
            .CheckDiskQuota(self.backend.as_ref(), self.disk_quota);
        if state.LargeEngineIDs.is_empty() && state.InProgressLargeEngines == 0 {
            return Ok(state);
        }
        if !state.LargeEngineIDs.is_empty() {
            let _guard = self
                .disk_quota_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.service
                .FlushAndImportLargeEngines(self.backend.as_ref(), &state.LargeEngineIDs)?;
        }
        Ok(state)
    }

    /// Start the quota loop used by IMPORT FROM SELECT. Stopping joins the worker,
    /// so final engine close/import cannot race with a quota-triggered import.
    pub fn StartDiskQuotaCheck(&self) -> DiskQuotaCheckHandle {
        let service = Arc::clone(&self.service);
        let backend = Arc::clone(&self.backend);
        let disk_quota = self.disk_quota;
        let disk_quota_lock = Arc::clone(&self.disk_quota_lock);
        start_disk_quota_check_with(CheckDiskQuotaInterval, move || {
            let state = service.CheckDiskQuota(backend.as_ref(), disk_quota);
            if state.LargeEngineIDs.is_empty() && state.InProgressLargeEngines == 0 {
                return Ok(());
            }
            if !state.LargeEngineIDs.is_empty() {
                let _guard = disk_quota_lock
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                // Match Go: a quota import failure is retried by a later check and
                // does not fail the foreground IMPORT FROM SELECT operation.
                return service.FlushAndImportLargeEngines(backend.as_ref(), &state.LargeEngineIDs);
            }
            Ok(())
        })
    }

    /// 绑定 IMPORT FROM SELECT 的选行 chunk 接收端。
    pub fn SetSelectedChunkCh(&mut self, receiver: mpsc::Receiver<QueryChunk>) {
        self.chunk_receiver = Some(Arc::new(Mutex::new(receiver)));
    }

    /// 从选行 channel 导入数据：打开引擎、处理 chunk、导入清理并做后处理。
    pub fn ImportSelectedRows(
        &mut self,
        context: &Context,
        group_checksum: Arc<Mutex<KVGroupChecksum>>,
    ) -> Result<i64, String> {
        let data_engine = self.OpenDataEngine(context, 1)?;
        let index_engine = self.OpenIndexEngine(context, IndexEngineID)?;
        let quota_checker = self.StartDiskQuotaCheck();
        let chunk = Chunk::default();
        let process_result = crate::ProcessChunk(
            context,
            &chunk,
            self,
            &data_engine,
            &index_engine,
            Some(Arc::clone(&group_checksum)),
            None,
        );
        // Stop and join before the final engine close/import, including the
        // ProcessChunk error path.
        quota_checker.Stop();
        process_result?;
        let closed_data = data_engine
            .Close(context)
            .map_err(|error| error.to_string())?;
        let data_count = self.ImportAndCleanup(context, &closed_data)?;
        let closed_index = index_engine
            .Close(context)
            .map_err(|error| error.to_string())?;
        self.ImportAndCleanup(context, &closed_index)?;
        let checksum = group_checksum
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        PostProcess(
            &self.service.AllocatorMaximums(),
            &self.LoadDataController.Plan,
            &checksum,
            self.service.as_ref(),
        )?;
        Ok(data_count)
    }

    /// 本地排序（sort）目录路径。
    pub fn SortDirectory(&self) -> &Path {
        &self.sort_directory
    }

    /// 导入任务标识（通常为 job id）。
    pub fn Identifier(&self) -> &str {
        &self.id
    }
}

/// Running disk-quota loop. `Stop` is idempotent and always joins its worker.
pub struct DiskQuotaCheckHandle {
    state: Arc<(Mutex<bool>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl DiskQuotaCheckHandle {
    pub fn Stop(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        let (stopped, wake) = &*self.state;
        *stopped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        wake.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for DiskQuotaCheckHandle {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

pub(crate) fn start_disk_quota_check_with(
    interval: Duration,
    mut check: impl FnMut() -> Result<(), String> + Send + 'static,
) -> DiskQuotaCheckHandle {
    let state = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_state = Arc::clone(&state);
    let worker = thread::spawn(move || {
        let (stopped, wake) = &*worker_state;
        loop {
            let guard = stopped
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (guard, _) = wake
                .wait_timeout_while(guard, interval, |stopped| !*stopped)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *guard {
                return;
            }
            drop(guard);
            // Match Go: quota import errors are logged there and retried on the
            // next tick instead of failing IMPORT FROM SELECT.
            let _ = check();
        }
    });
    DiskQuotaCheckHandle {
        state,
        worker: Some(worker),
    }
}

/// 将 TableImporter 适配为 chunk 处理运行时接口。
impl TableImporterRuntime for TableImporter {
    fn DataSourceType(&self) -> DataSourceType {
        self.LoadDataController.Plan.DataSourceType
    }

    fn TableInfo(&self) -> &TableInfo {
        &self.table_info
    }

    fn GetKeySpace(&self) -> Vec<u8> {
        TableImporter::GetKeySpace(self)
    }

    fn GetKVEncoder(&self, chunk: &dyn ImportChunk) -> Result<TableKVEncoder, String> {
        let chunk = Chunk {
            Path: chunk.Path().to_owned(),
            FileSize: chunk.FileSize(),
            Offset: chunk.Offset(),
            EndOffset: chunk.EndOffset(),
            PrevRowIDMax: chunk.PrevRowIDMax(),
            RowIDMax: chunk.RowIDMax(),
            Type: chunk.SourceType(),
            Compression: chunk.Compression(),
            Timestamp: chunk.Timestamp(),
            ..Chunk::default()
        };
        self.getKVEncoder(&chunk)
    }

    fn GetParser(
        &self,
        _context: &Context,
        chunk: &dyn ImportChunk,
    ) -> Result<Box<dyn Parser + Send>, String> {
        let location = chunk
            .ParquetLocation()
            .unwrap_or_else(|| self.LoadDataController.ParquetLocation());
        let chunk = Chunk {
            Path: chunk.Path().to_owned(),
            FileSize: chunk.FileSize(),
            Offset: chunk.Offset(),
            EndOffset: chunk.EndOffset(),
            PrevRowIDMax: chunk.PrevRowIDMax(),
            RowIDMax: chunk.RowIDMax(),
            Type: chunk.SourceType(),
            Compression: chunk.Compression(),
            Timestamp: chunk.Timestamp(),
            ..Chunk::default()
        };
        let mut parser = if chunk.Type == SourceType::Parquet {
            let file = SourceFileMeta {
                path: chunk.Path.clone(),
                file_size: chunk.FileSize,
                source_type: SourceType::Parquet,
                compression: chunk.Compression,
                ..Default::default()
            };
            self.LoadDataController.OpenParquetParserWithLocation(
                &Default::default(),
                &file,
                location,
            )?
        } else {
            self.service
                .NewParserWithParquetLocation(&self.LoadDataController, &chunk, location)?
        };
        // 文件起点：跳过 IgnoreLines 并设置起始 RowID；中间 chunk 直接 Seek。
        if chunk.Offset == 0 {
            crate::HandleSkipNRows(parser.as_mut(), self.LoadDataController.Plan.IgnoreLines)?;
            parser.SetRowID(chunk.PrevRowIDMax);
        } else {
            parser
                .SetPos(chunk.Offset, chunk.PrevRowIDMax)
                .map_err(|error| error.to_string())?;
        }
        Ok(parser)
    }

    fn TakeQueryChunks(&self) -> Result<SharedQueryChunkReceiver, String> {
        self.chunk_receiver
            .clone()
            .ok_or_else(|| "selected-row chunk channel is not configured".into())
    }
}

impl LoadDataController {
    /// 按真实体量与引擎上限估算子任务数；Global Sort 时再对齐到执行节点数倍数。
    pub fn calculateSubtaskCnt(&self) -> usize {
        calculateSubtaskCnt(
            self.TotalRealSize,
            self.Plan.MaxEngineSize.0,
            self.Plan.IsGlobalSort(),
            self.ExecuteNodesCnt,
        )
    }

    /// 按子任务数均分后的每引擎目标大小（向上取整）。
    pub fn getAdjustedMaxEngineSize(&self) -> i64 {
        getAdjustedMaxEngineSize(
            self.TotalRealSize,
            self.Plan.MaxEngineSize.0,
            self.Plan.IsGlobalSort(),
            self.ExecuteNodesCnt,
        )
    }

    /// 设置可参与执行的节点数量。
    pub fn SetExecuteNodeCnt(&mut self, count: usize) {
        self.ExecuteNodesCnt = count;
    }

    /// 将表 Region 划分结果展开为按 engine ID 分组的 Chunk 列表。
    pub fn PopulateChunks(
        &self,
        service: &dyn TableImporterService,
    ) -> Result<HashMap<i32, Vec<Chunk>>, String> {
        let regions = service.MakeTableRegions(self, self.getAdjustedMaxEngineSize())?;
        let timestamp = unix_timestamp();
        let mut chunks = HashMap::<i32, Vec<Chunk>>::new();
        for region in regions {
            chunks.entry(region.EngineID).or_default().push(Chunk {
                Path: region.File.path,
                FileSize: region.File.file_size,
                Offset: region.Offset,
                EndOffset: region.EndOffset,
                PrevRowIDMax: region.PrevRowIDMax,
                RowIDMax: region.RowIDMax,
                Type: region.File.source_type,
                Compression: region.File.compression,
                Timestamp: timestamp,
            });
        }
        // 确保索引引擎槽位存在，即使当前无索引 chunk。
        chunks.entry(IndexEngineID).or_default();
        Ok(chunks)
    }
}

pub(crate) fn calculateSubtaskCnt(
    total_real_size: i64,
    max_engine_size: i64,
    global_sort: bool,
    execute_nodes_cnt: usize,
) -> usize {
    let maximum = max_engine_size.max(1);
    let mut count = if total_real_size <= maximum {
        1
    } else {
        (total_real_size as f64 / maximum as f64).round().max(1.0) as usize
    };
    if global_sort && execute_nodes_cnt > 0 {
        count = count.div_ceil(execute_nodes_cnt) * execute_nodes_cnt;
    }
    count
}

pub(crate) fn getAdjustedMaxEngineSize(
    total_real_size: i64,
    max_engine_size: i64,
    global_sort: bool,
    execute_nodes_cnt: usize,
) -> i64 {
    (total_real_size as f64
        / calculateSubtaskCnt(
            total_real_size,
            max_engine_size,
            global_sort,
            execute_nodes_cnt,
        ) as f64)
        .ceil() as i64
}

/// 将用户配额限制在磁盘容量的 80% 以内；未配置时用默认配额。
pub fn adjustDiskQuota(
    disk_quota: i64,
    sort_directory: &Path,
    service: &dyn TableImporterService,
) -> i64 {
    let capacity = match service.DiskCapacity(sort_directory) {
        Ok(capacity) => capacity,
        Err(_) if disk_quota != 0 => return disk_quota,
        Err(_) => return crate::DefaultDiskQuota.0,
    };
    let maximum = (capacity as f64 * 0.8) as i64;
    if disk_quota == 0 || disk_quota > maximum {
        maximum
    } else {
        disk_quota
    }
}

/// 导入后处理：rebase 自增分配器基址，并按计划校验本地/远程 checksum。
pub fn PostProcess(
    maximum_ids: &HashMap<AllocatorType, i64>,
    plan: &Plan,
    local_checksum: &KVGroupChecksum,
    service: &dyn TableImporterService,
) -> Result<(), String> {
    RebaseAllocatorBases(maximum_ids, plan, service)?;
    VerifyChecksum(plan, &local_checksum.MergedChecksum(), || {
        RemoteChecksumTableBySQL(plan, service)
    })
}

/// 若表含 AutoRowID/自增/AutoRandom，则将分配器基址推进到导入最大值之后。
pub fn RebaseAllocatorBases(
    maximum_ids: &HashMap<AllocatorType, i64>,
    plan: &Plan,
    service: &dyn TableImporterService,
) -> Result<(), String> {
    if plan.DesiredTableInfo.as_ref().is_none_or(|table| {
        let has_auto_row_id = !table.PKIsHandle && !table.IsCommonHandle;
        !(has_auto_row_id
            || table.GetAutoIncrementColInfo().is_some()
            || table.ContainsAutoRandomBits())
    }) {
        return Ok(());
    }
    service.RebaseAllocatorBases(maximum_ids, plan)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 通过 SQL 查询得到的远端表校验和摘要。
pub struct RemoteChecksum {
    pub Schema: String,
    pub Table: String,
    pub Checksum: u64,
    pub TotalKVs: u64,
    pub TotalBytes: u64,
}

impl RemoteChecksum {
    /// 与本地 KVChecksum 在校验和、KV 数与字节数上是否一致。
    pub fn IsEqual(&self, local: &KVChecksum) -> bool {
        self.Checksum == local.Sum()
            && self.TotalKVs == local.SumKVS()
            && self.TotalBytes == local.SumSize()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 远端校验和查询错误；`Retryable` 表示可降并发重试。
pub struct RemoteChecksumError {
    pub Message: String,
    pub Retryable: bool,
}

/// 按 PostOpLevel 执行校验：Off 跳过，Optional 失败可忽略，Required 必须匹配。
pub fn VerifyChecksum(
    plan: &Plan,
    local_checksum: &KVChecksum,
    get_remote_checksum: impl FnOnce() -> Result<RemoteChecksum, String>,
) -> Result<(), String> {
    if plan.Checksum == PostOpLevel::Off {
        return Ok(());
    }
    let remote = match get_remote_checksum() {
        Ok(remote) => remote,
        Err(_) if plan.Checksum == PostOpLevel::Optional => return Ok(()),
        Err(error) => return Err(error),
    };
    if remote.IsEqual(local_checksum) {
        return Ok(());
    }
    let error = format!(
        "checksum mismatched remote vs local => (checksum: {} vs {}) (total_kvs: {} vs {}) (total_bytes:{} vs {})",
        remote.Checksum,
        local_checksum.Sum(),
        remote.TotalKVs,
        local_checksum.SumKVS(),
        remote.TotalBytes,
        local_checksum.SumSize(),
    );
    if plan.Checksum == PostOpLevel::Optional {
        Ok(())
    } else {
        Err(error)
    }
}

/// 带退避的远端校验和查询：可重试错误时减半 DistSQL 并发最多重试 3 次。
pub fn RemoteChecksumTableBySQL(
    plan: &Plan,
    service: &dyn TableImporterService,
) -> Result<RemoteChecksum, String> {
    let mut factor = 1_usize;
    let mut last_error = None;
    // 每次可重试失败后将并发减半（factor 翻倍）。
    for _ in 0..3 {
        let concurrency = (plan.DistSQLScanConcurrency / factor)
            .max(astersql_ingestor_ingestctrl::checksum::MinDistSQLScanConcurrency);
        match service.RemoteChecksumTableBySQL(plan, concurrency, GetBackoffWeight(plan)) {
            Ok(checksum) => return Ok(checksum),
            Err(error) if error.Retryable => {
                last_error = Some(error.Message);
                factor = factor.saturating_mul(2);
            }
            Err(error) => return Err(error.Message),
        }
    }
    Err(last_error.unwrap_or_else(|| "remote checksum failed".into()))
}

/// 读取 `tidb_backoff_weight`，至少为默认值 2。
pub fn GetBackoffWeight(plan: &Plan) -> i32 {
    const DEFAULT_BACKOFF_WEIGHT: i32 =
        astersql_ingestor_ingestctrl::checksum::DefaultBackoffWeight;
    plan.ImportantSysVars
        .get("tidb_backoff_weight")
        .and_then(|value| value.parse::<i32>().ok())
        .map_or(DEFAULT_BACKOFF_WEIGHT, |value| {
            value.max(DEFAULT_BACKOFF_WEIGHT)
        })
}

/// 构造 import 根目录：`{TempDir}/import-{Port}`。
pub fn GetImportRootDir(configuration: &ImportRuntimeConfig) -> PathBuf {
    configuration
        .TempDir
        .join(format!("import-{}", configuration.Port))
}

/// 将导入行数刷入表统计信息。
pub fn FlushTableStats(
    table_id: i64,
    imported_rows: i64,
    service: &dyn TableImporterService,
) -> Result<(), String> {
    service.FlushTableStats(table_id, imported_rows)
}

/// 当前 Unix 秒级时间戳，失败时返回 0。
fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

/// Existing-store allocator discovery preserves proxy endpoints for global groups.
pub fn newEtcdClientForAllocatorRebase(
    context: &astersql_metaservice::Context,
    store: Option<&dyn astersql_metaservice::EtcdMetadataStore>,
    caller_endpoints: &[String],
    config: astersql_metaservice::EtcdDialConfig,
) -> Result<astersql_metaservice::NamespacedEtcdClient, String> {
    let store = store.ok_or_else(|| "TiKV store does not expose PD client".to_owned())?;
    astersql_metaservice::NewEtcdClientFromStore(context, store, caller_endpoints, config)
        .map_err(|error| error.to_string())
}
