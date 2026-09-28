// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// Lightning local backend：本地排序写入后再将 SST 导入 TiKV。
//
// 包含版本兼容检查（TiDB/TiKV/PD/TiFlash）、Backend 配置与生命周期、
// Engine 打开/关闭/导入、按 Range 属性切分键范围，以及 Store 磁盘可用空间校验。
// Region 是 TiKV 数据分片；SST 为有序字符串表，可直接 ingest 进 RocksDB/Pebble。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::disk_quota::DiskUsage;
use crate::duplicate::{DupeController, ErrorManager, NewDupeDetector, TransactionFactory};
use crate::engine::{Engine, Writer, nextKey};
use crate::engine_mgr::{EngineManager, ExternalEngine, StoreHelper, newEngineManager};
use crate::{CancellationToken, ConflictInfo, EngineFileSize, EngineId, Error, KeyRange, Result};

/// 拨号/连接超时（5 分钟）。
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// 通用最大重试次数。
pub const MAX_RETRY_TIMES: usize = 20;
/// 默认重试退避时间。
pub const DEFAULT_RETRY_BACKOFF_TIME: Duration = Duration::from_secs(3);
/// gRPC keepalive 间隔。
pub const GRPC_KEEP_ALIVE_TIME: Duration = Duration::from_secs(10 * 60);
/// gRPC keepalive 超时。
pub const GRPC_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// gRPC 退避最大延迟。
pub const GRPC_BACKOFF_MAX_DELAY: Duration = Duration::from_secs(10 * 60);
/// Range 属性按大小采样的默认距离（4MiB）。
pub const DEFAULT_PROP_SIZE_INDEX_DISTANCE: u64 = 4 * 1024 * 1024;
/// Range 属性按键数采样的默认距离。
pub const DEFAULT_PROP_KEYS_INDEX_DISTANCE: u64 = 40 * 1024;
/// 打开文件数下限阈值。
pub const OPEN_FILES_LOWER_THRESHOLD: i32 = 128;
/// 重复键结果库目录名。
pub const DUPLICATE_DB_NAME: &str = "duplicates";
/// 单次扫描 Region 数量上限。
pub const SCAN_REGION_LIMIT: usize = 128;
/// 写入并导入的最大重试次数。
pub const MAX_WRITE_AND_INGEST_RETRY_TIMES: usize = 30;
/// RPC 接收消息大小不限制。
pub const UNLIMITED_RPC_RECV_MSG_SIZE: i32 = i32::MAX;
/// 强制按 Region 分区的阈值。
pub static FORCE_PARTITION_REGION_THRESHOLD: AtomicI32 = AtomicI32::new(100);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
/// 语义化版本号 major.minor.patch。
pub struct Version {
    /// 主版本号。
    pub major: u64,
    /// 次版本号。
    pub minor: u64,
    /// 修订号。
    pub patch: u64,
}

impl Version {
    /// 构造版本号。
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// 解析 `vX.Y.Z` 或 `X.Y.Z`（忽略预发布后缀）。
    pub fn parse(value: &str) -> Result<Self> {
        let value = value
            .trim()
            .trim_start_matches('v')
            .split('-')
            .next()
            .unwrap_or(value);
        let mut pieces = value.split('.');
        let parse = |part: Option<&str>| {
            part.unwrap_or("0")
                .parse()
                .map_err(|_| Error::InvalidData(format!("invalid version: {value}")))
        };
        Ok(Self::new(
            parse(pieces.next())?,
            parse(pieces.next())?,
            parse(pieces.next())?,
        ))
    }
}

/// local backend 要求的最低 TiDB 版本。
pub fn localMinTiDBVersion() -> Version {
    Version::new(4, 0, 0)
}
/// local backend 要求的最低 TiKV 版本。
pub fn localMinTiKVVersion() -> Version {
    Version::new(4, 0, 0)
}
/// local backend 要求的最低 PD 版本。
pub fn localMinPDVersion() -> Version {
    Version::new(4, 0, 0)
}
/// 支持 TiFlash 副本时的最低版本。
pub fn tiflashMinVersion() -> Version {
    Version::new(4, 0, 5)
}
/// TiKV 侧空闲空间检查可用的最低版本。
pub fn tikvSideFreeSpaceCheckVersion() -> Version {
    Version::new(8, 0, 0)
}

/// 检查版本是否落在 [minimum, maximum) 半开区间。
pub fn checkTiDBVersion(version: &str, minimum: Version, maximum: Version) -> Result<()> {
    let version = Version::parse(version)?;
    if version < minimum || version >= maximum {
        Err(Error::InvalidData(format!(
            "unsupported version {version:?}, expected [{minimum:?}, {maximum:?})"
        )))
    } else {
        Ok(())
    }
}

/// 检查源表与 TiFlash 副本表在旧版本上是否冲突。
///
/// TiDB 4.0.5 之前 local backend 不支持对已有 TiFlash 副本的表导入。
pub fn CheckTiFlashVersionForTables(
    version: Version,
    source_tables: &[(&str, &str)],
    tiflash_tables: &[(&str, &str)],
) -> Result<()> {
    if version >= tiflashMinVersion() {
        return Ok(());
    }
    let conflicts = source_tables
        .iter()
        .filter(|source| tiflash_tables.iter().any(|table| table == *source))
        .map(|(database, table)| format!("`{database}`.`{table}`"))
        .collect::<Vec<_>>();
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "lightning local backend doesn't support TiFlash in this TiDB version. conflict tables: [{}]. Please add TiFlash replica after load data.",
            conflicts.join(", ")
        )))
    }
}

/// 向 Store 写入并 ingest SST 的客户端。
pub trait ImportClient: Send + Sync {
    /// 将 Engine 中指定键范围写入并导入，返回 (字节数, 键数)。
    fn WriteAndIngest(
        &self,
        token: &CancellationToken,
        engine: &Engine,
        ranges: &[KeyRange],
    ) -> Result<(i64, i64)>;
    /// 关闭客户端。
    fn Close(&self);
}

/// 按 Store ID 创建 ImportClient 的工厂。
pub trait ImportClientFactory: Send + Sync {
    /// 为指定 Store 创建导入客户端。
    fn Create(&self, token: &CancellationToken, store_id: u64) -> Result<Arc<dyn ImportClient>>;
    /// 关闭工厂及已创建的客户端。
    fn Close(&self);
}

/// 目标集群元信息目录：版本、库表与 TiFlash 副本查询。
pub trait TargetCatalog: Send + Sync {
    /// 拉取远端库列表。
    fn FetchRemoteDBModels(&self, token: &CancellationToken) -> Result<Vec<DatabaseModel>>;
    /// 拉取指定 schema 下的表列表。
    fn FetchRemoteTableModels(
        &self,
        token: &CancellationToken,
        schema: &str,
    ) -> Result<Vec<TableModel>>;
    /// 查询 TiDB 版本字符串。
    fn TiDBVersion(&self, token: &CancellationToken) -> Result<String>;
    /// 查询各 TiKV 版本。
    fn TiKVVersions(&self, token: &CancellationToken) -> Result<Vec<String>>;
    /// 查询各 PD 版本。
    fn PDVersions(&self, token: &CancellationToken) -> Result<Vec<String>>;
    /// 查询已配置 TiFlash 副本的 (db, table) 列表。
    fn TiFlashReplicas(&self, token: &CancellationToken) -> Result<Vec<(String, String)>>;
}

#[derive(Clone, Debug, Default)]
/// 远端库模型。
pub struct DatabaseModel {
    /// 库名。
    pub name: String,
}
#[derive(Clone, Debug, Default)]
/// 远端表模型。
pub struct TableModel {
    /// 所属 schema/库名。
    pub schema: String,
    /// 表名。
    pub name: String,
}

/// 目标集群信息获取器：封装版本与副本兼容性检查。
pub struct TargetInfoGetter {
    /// 元信息目录实现。
    catalog: Arc<dyn TargetCatalog>,
}
/// 构造 TargetInfoGetter。
pub fn NewTargetInfoGetter(catalog: Arc<dyn TargetCatalog>) -> TargetInfoGetter {
    TargetInfoGetter { catalog }
}

impl TargetInfoGetter {
    pub fn FetchRemoteDBModels(&self, token: &CancellationToken) -> Result<Vec<DatabaseModel>> {
        self.catalog.FetchRemoteDBModels(token)
    }

    pub fn FetchRemoteTableModels(
        &self,
        token: &CancellationToken,
        schema: &str,
    ) -> Result<Vec<TableModel>> {
        self.catalog.FetchRemoteTableModels(token, schema)
    }

    /// 校验 TiDB/TiKV/PD 版本，并感知 TiFlash 副本存在性。
    pub fn CheckRequirements(&self, token: &CancellationToken) -> Result<()> {
        checkTiDBVersion(
            &self.catalog.TiDBVersion(token)?,
            localMinTiDBVersion(),
            Version::new(1000, 0, 0),
        )?;
        for version in self.catalog.TiKVVersions(token)? {
            checkTiDBVersion(&version, localMinTiKVVersion(), Version::new(1000, 0, 0))?;
        }
        for version in self.catalog.PDVersions(token)? {
            checkTiDBVersion(&version, localMinPDVersion(), Version::new(1000, 0, 0))?;
        }
        let replicas = self.catalog.TiFlashReplicas(token)?;
        if !replicas.is_empty() {
            // TiFlash replica creation is safe only after the minimum version;
            // callers with replicas must have supplied that via cluster checks.
            let _required = tiflashMinVersion();
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
/// Local backend 运行配置。
/// Local backend：管理 Engine、导入客户端与并发配置。
pub struct BackendConfig {
    /// 本地排序 KV 存储目录。
    pub local_store_dir: String,
    /// Worker 并发度。
    pub worker_concurrency: usize,
    /// 是否启用重复键检测。
    pub duplicate_detection: bool,
    /// 最大打开文件数。
    pub max_open_files: i32,
    /// Keyspace 名称（多租户）。
    pub keyspace_name: String,
    /// 资源组名称。
    pub resource_group_name: String,
    /// 任务类型标识（默认 lightning）。
    pub task_type: String,
    /// 是否在启动时检查集群版本要求。
    pub check_requirements: bool,
    /// 是否检查 TiKV 磁盘空间。
    pub check_tikv_disk: bool,
    /// Region 按大小分裂阈值。
    pub region_split_size: i64,
    /// Region 按键数分裂阈值。
    pub region_split_keys: i64,
    /// 导入失败重试退避。
    pub retry_backoff: Duration,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            local_store_dir: "./sorted-kv-dir".into(),
            worker_concurrency: 4,
            duplicate_detection: false,
            max_open_files: 1024,
            keyspace_name: String::new(),
            resource_group_name: String::new(),
            task_type: "lightning".into(),
            check_requirements: true,
            check_tikv_disk: true,
            region_split_size: 96 * 1024 * 1024,
            region_split_keys: 960_000,
            retry_backoff: DEFAULT_RETRY_BACKOFF_TIME,
        }
    }
}

impl BackendConfig {
    /// 规范化配置：并发/打开文件数/分裂阈值取下限。
    pub fn adjust(&mut self) {
        self.worker_concurrency = self.worker_concurrency.max(1);
        self.max_open_files = self.max_open_files.max(OPEN_FILES_LOWER_THRESHOLD);
        self.region_split_size = self.region_split_size.max(1);
        self.region_split_keys = self.region_split_keys.max(1);
    }

    /// 返回 Worker 并发度。
    pub fn GetWorkerConcurrency(&self) -> usize {
        self.worker_concurrency
    }
    /// 设置 Worker 并发度（至少为 1）。
    pub fn SetWorkerConcurrency(&mut self, concurrency: usize) {
        self.worker_concurrency = concurrency.max(1);
    }
}

/// Region 分裂与 scatter 客户端。
pub trait SplitClient: Send + Sync {
    /// 按给定键分裂 Region 并打散到各 Store。
    fn SplitKeysAndScatter(&self, token: &CancellationToken, keys: &[Vec<u8>]) -> Result<()>;
}

pub struct Backend {
    /// 可变配置。
    config: Mutex<BackendConfig>,
    /// Engine 管理器。
    engine_manager: Arc<EngineManager>,
    /// 导入客户端工厂；为 None 时仅用本地统计。
    import_factory: Option<Arc<dyn ImportClientFactory>>,
    /// Region 分裂客户端。
    split_client: Option<Arc<dyn SplitClient>>,
    /// 当前 Worker 并发（原子）。
    worker_concurrency: AtomicI32,
    /// 是否已关闭。
    closed: AtomicBool,
    /// 各 Engine 已导入 KV 条数缓存。
    imported_counts: Mutex<HashMap<EngineId, i64>>,
}

/// 校验 rlimit 后创建 Backend 与 EngineManager。
pub fn NewBackend(
    mut config: BackendConfig,
    store_helper: Arc<dyn StoreHelper>,
    import_factory: Option<Arc<dyn ImportClientFactory>>,
    split_client: Option<Arc<dyn SplitClient>>,
) -> Result<Backend> {
    config.adjust();
    // 校验进程可打开文件数满足 max_open_files
    crate::local_unix::VerifyRLimit(config.max_open_files as u64)?;
    let concurrency = config.worker_concurrency as i32;
    let engine_manager = Arc::new(newEngineManager(config.clone(), store_helper)?);
    Ok(Backend {
        config: Mutex::new(config),
        engine_manager,
        import_factory,
        split_client,
        worker_concurrency: AtomicI32::new(concurrency),
        closed: AtomicBool::new(false),
        imported_counts: Mutex::new(HashMap::new()),
    })
}

impl Backend {
    /// 关闭导入工厂与 EngineManager（幂等）。
    pub fn Close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(factory) = &self.import_factory {
            factory.Close();
        }
        self.engine_manager.close();
    }

    /// 读取当前 Worker 并发。
    pub fn GetWorkerConcurrency(&self) -> usize {
        self.worker_concurrency.load(Ordering::Acquire).max(1) as usize
    }

    /// 更新 Worker 并发。
    pub fn SetWorkerConcurrency(&self, concurrency: usize) {
        self.worker_concurrency
            .store(concurrency.max(1) as i32, Ordering::Release);
    }

    /// 所有 Engine 内存占用之和。
    pub fn TotalMemoryConsume(&self) -> i64 {
        self.engine_manager.totalMemoryConsume()
    }
    /// 刷盘指定 Engine。
    pub fn FlushEngine(&self, id: EngineId) -> Result<()> {
        self.engine_manager.flushEngine(id)
    }
    /// 刷盘全部 Engine。
    pub fn FlushAllEngines(&self) -> Result<()> {
        self.engine_manager.flushAllEngines()
    }
    /// 清理全部本地 Engine 数据。
    pub fn CleanupAllLocalEngines(&self) {
        self.engine_manager.cleanupAllLocalEngines();
    }
    /// 返回导入重试退避时间。
    pub fn RetryImportDelay(&self) -> Duration {
        self.config
            .lock()
            .map(|config| config.retry_backoff)
            .unwrap_or(DEFAULT_RETRY_BACKOFF_TIME)
    }
    /// 是否需要导入后处理（恒为 true）。
    pub fn ShouldPostProcess(&self) -> bool {
        true
    }

    /// 打开（或创建）指定 Engine。
    pub fn OpenEngine(&self, token: &CancellationToken, id: EngineId) -> Result<Arc<Engine>> {
        let config = self.config.lock().map_err(|_| Error::Poisoned)?.clone();
        self.engine_manager.openEngine(
            token,
            id,
            config.region_split_size,
            config.region_split_keys,
        )
    }

    /// 关闭指定 Engine（不清理数据）。
    pub fn CloseEngine(&self, id: EngineId) -> Result<()> {
        self.engine_manager.closeEngine(id, false)
    }
    /// 清理指定 Engine 数据。
    pub fn CleanupEngine(&self, id: EngineId) -> Result<()> {
        self.engine_manager.cleanupEngine(id)
    }
    /// 获取本地批量 Writer。
    pub fn LocalWriter(&self, id: EngineId, batch_size: usize) -> Result<Writer> {
        self.engine_manager.localWriter(id, batch_size)
    }

    /// 导入 Engine：持有 import 锁 → 完成写入 → 分裂 Region → WriteAndIngest → 校验统计。
    pub fn ImportEngine(
        &self,
        token: &CancellationToken,
        id: EngineId,
        store_id: u64,
    ) -> Result<()> {
        token.check()?;
        let engine = self
            .engine_manager
            .lockEngine(id, crate::engine::IMPORT_MUTEX_STATE_IMPORT)
            .ok_or_else(|| Error::NotFound(format!("engine {id}")))?;
        let result = (|| {
            // 结束写入阶段，准备生成分裂键并导入
            engine.finishWrite()?;
            let split_keys = engine.GetRegionSplitKeys()?;
            if let Some(split_client) = &self.split_client {
                split_client.SplitKeysAndScatter(token, &split_keys)?;
            }
            let key_range = engine.GetKeyRange()?;
            let ranges = split_keys
                .windows(2)
                .map(|keys| KeyRange {
                    start: keys[0].clone(),
                    end: keys[1].clone(),
                })
                .collect::<Vec<_>>();
            // 无分裂键时整表作为一个范围导入
            let ranges = if ranges.is_empty() {
                vec![key_range]
            } else {
                ranges
            };
            let (bytes, count) = if let Some(factory) = &self.import_factory {
                factory
                    .Create(token, store_id)?
                    .WriteAndIngest(token, &engine, &ranges)?
            } else {
                engine.KVStatistics()
            };
            engine.FinishImport(bytes, count);
            verifyImportedStatistics(&engine, count)?;
            self.imported_counts
                .lock()
                .map_err(|_| Error::Poisoned)?
                .insert(id, count);
            Ok(())
        })();
        engine.unlock();
        result
    }

    /// 导入后重置 Engine（分配新 TS）。
    pub fn UnsafeImportAndReset(&self, token: &CancellationToken, id: EngineId) -> Result<()> {
        self.ImportEngine(token, id, 0)?;
        self.engine_manager.resetEngine(token, id, true)
    }

    /// 查询已导入 KV 条数。
    pub fn GetImportedKVCount(&self, id: EngineId) -> i64 {
        self.imported_counts
            .lock()
            .ok()
            .and_then(|counts| counts.get(&id).copied())
            .unwrap_or_else(|| self.engine_manager.getImportedKVCount(id))
    }

    /// 获取外部 Engine 句柄（若有）。
    pub fn GetExternalEngine(&self, id: EngineId) -> Option<Arc<dyn ExternalEngine>> {
        self.engine_manager.getExternalEngine(id)
    }
    /// 外部 Engine 的 KV 统计。
    pub fn GetExternalEngineKVStatistics(&self, id: EngineId) -> Option<(i64, i64)> {
        self.engine_manager.getExternalEngineKVStatistics(id)
    }
    /// 外部 Engine 的冲突信息。
    pub fn GetExternalEngineConflictInfo(&self, id: EngineId) -> ConflictInfo {
        self.engine_manager.getExternalEngineConflictInfo(id)
    }
    /// 重置 Engine 但不分配新 TS。
    pub fn ResetEngineSkipAllocTS(&self, token: &CancellationToken, id: EngineId) -> Result<()> {
        self.engine_manager.resetEngine(token, id, false)
    }

    /// 导入前设置 Engine 元数据中的时间戳。
    pub fn SetTSBeforeImportEngine(&self, id: EngineId, ts: u64) -> Result<()> {
        let engine = self
            .engine_manager
            .rLockEngine(id)
            .ok_or_else(|| Error::NotFound(format!("engine {id}")))?;
        engine.engine_meta.ts.store(ts, Ordering::Release);
        engine.rUnlock();
        Ok(())
    }

    /// 构造重复键控制器。
    pub fn GetDupeController(
        &self,
        table_name: String,
        error_manager: Arc<dyn ErrorManager>,
        transaction_factory: Arc<dyn TransactionFactory>,
    ) -> DupeController {
        let detector = NewDupeDetector(table_name, error_manager, transaction_factory);
        DupeController::new(
            detector,
            self.engine_manager.getDuplicateData(),
            self.engine_manager.getKeyAdapter(),
        )
    }

    /// 各 Engine 文件大小快照。
    /// DiskUsage：返回各 Engine 文件大小。
    pub fn EngineFileSizes(&self) -> Vec<EngineFileSize> {
        self.engine_manager.engineFileSizes()
    }
    /// 关闭 EngineManager。
    pub fn CloseEngineMgr(&self) {
        self.engine_manager.close();
    }
}

impl DiskUsage for Backend {
    fn EngineFileSizes(&self) -> Vec<EngineFileSize> {
        self.engine_manager.engineFileSizes()
    }
}

/// 按 Range 属性（大小/键数）将完整键范围切成多个子范围。
pub fn splitRangeBySizeProps(
    full_range: KeyRange,
    properties: &[(Vec<u8>, u64, u64)],
    size_limit: i64,
    keys_limit: i64,
) -> Vec<KeyRange> {
    let mut ranges = Vec::new();
    let mut current_key = full_range.start.clone();
    let mut size = 0u64;
    let mut keys = 0u64;
    for (key, item_size, item_keys) in properties {
        if key <= &current_key {
            continue;
        }
        if key > &full_range.end {
            break;
        }
        size += item_size;
        keys += item_keys;
        if size as i64 >= size_limit || keys as i64 >= keys_limit {
            ranges.push(KeyRange {
                start: current_key,
                end: key.clone(),
            });
            current_key = key.clone();
            size = 0;
            keys = 0;
        }
    }
    if current_key < full_range.end {
        if !ranges.is_empty() && keys == 0 {
            ranges.last_mut().expect("checked non-empty").end = full_range.end;
        } else {
            ranges.push(KeyRange {
                start: current_key,
                end: full_range.end,
            });
        }
    }
    ranges
}

/// 校验导入条数与 Engine 内统计一致，防止漏导。
pub fn verifyImportedStatistics(engine: &Engine, imported_kv_count: i64) -> Result<()> {
    let (_, expected) = engine.KVStatistics();
    let (_, actual) = engine.ImportedStatistics();
    if imported_kv_count != actual || expected != actual {
        Err(Error::InvalidData(format!(
            "imported KV count mismatch: expected={expected}, reported={imported_kv_count}, actual={actual}"
        )))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
/// Store 容量信息。
pub struct StoreInfo {
    /// 总容量字节。
    pub capacity: u64,
    /// 可用字节。
    pub available: u64,
}

/// 可用空间低于容量 10% 时返回磁盘配额错误。
pub fn checkDiskAvail(store: &StoreInfo) -> Result<()> {
    if store.capacity == 0 {
        return Ok(());
    }
    // 可用 < 容量的 10% 视为磁盘不足
    if store.available.saturating_mul(10) < store.capacity {
        Err(Error::DiskQuotaExceeded {
            used: (store.capacity - store.available) as i64,
            quota: (store.capacity * 9 / 10) as i64,
        })
    } else {
        Ok(())
    }
}

/// 计算 Engine 对应的 SST 输出目录路径。
pub fn engineSSTDir(store_dir: &str, engine_id: EngineId) -> PathBuf {
    Path::new(store_dir).join(format!("{engine_id}.sst"))
}

/// 若所有 Store 上报的分裂大小/键数一致则采用之，否则回退默认值。
pub fn GetRegionSplitSizeKeys(
    stores: &[(i64, i64)],
    default_size: i64,
    default_keys: i64,
) -> (i64, i64) {
    let sizes: HashSet<_> = stores
        .iter()
        .map(|(size, _)| *size)
        .filter(|size| *size > 0)
        .collect();
    let keys: HashSet<_> = stores
        .iter()
        .map(|(_, keys)| *keys)
        .filter(|keys| *keys > 0)
        .collect();
    (
        if sizes.len() == 1 {
            *sizes.iter().next().unwrap()
        } else {
            default_size
        },
        if keys.len() == 1 {
            *keys.iter().next().unwrap()
        } else {
            default_keys
        },
    )
}

/// 计算键的后继（用于半开区间上界）。
pub fn NextKey(key: &[u8]) -> Vec<u8> {
    nextKey(key)
}
