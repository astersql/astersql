// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// unistore 嵌入式 Server：打开引擎、引导 Region，并组装 StandAlone 服务。
//
// 提供 `new_mock`（内置 MockPd/MockRegionManager）与 `new`（外部 PdClient）
// 两条建服路径；Raft 子路径与 RaftStore 模式在此暂不支持。

use astersql_store_mockstore_unistore_config::{CompressionType, Config, Engine, ParseCompression};
use astersql_store_mockstore_unistore_lockstore::MemStore;
use astersql_store_mockstore_unistore_tikv::inner_server::{
    DatabaseBundle, InnerServer, StandAloneInnerServer,
};
use astersql_store_mockstore_unistore_tikv::mock_region::{
    MockPd, MockRegionManager, Peer, Region, RegionEpoch, RegionError, Store,
};
use astersql_store_mockstore_unistore_tikv::mvcc::{MvccStore, SafePoint};
use astersql_store_mockstore_unistore_tikv::region::{
    Latches, RegionContext, RegionManager, RegionOptions, RequestContext, StandAloneRegionManager,
};
use astersql_store_mockstore_unistore_tikv::server::Server;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// Raft 引擎子目录名（当前 create_db 不支持打开）。
pub const SUB_PATH_RAFT: &str = "raft";
/// KV 引擎子目录名。
pub const SUB_PATH_KV: &str = "kv";
/// 锁存储 arena 大小（8 MiB）。
const LOCK_STORE_ARENA_SIZE: usize = 8 << 20;
/// Badger 风格 LSM 层数，压缩配置必须至少这么多级。
const BADGER_LEVEL_COUNT: usize = 7;

/// Server 启动与配置相关错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerError {
    Unsupported(String),
    InvalidConfig(String),
    Io(String),
    Pd(String),
    Region(RegionError),
    Start(String),
}

impl Display for ServerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(value) => write!(f, "not support {value}"),
            Self::InvalidConfig(value) => write!(f, "invalid config: {value}"),
            Self::Io(value) => write!(f, "open database failed: {value}"),
            Self::Pd(value) => write!(f, "PD request failed: {value}"),
            Self::Region(value) => write!(f, "region setup failed: {value}"),
            Self::Start(value) => write!(f, "inner server start failed: {value}"),
        }
    }
}

impl std::error::Error for ServerError {}

/// 建服所需的最小 PD 能力：取时间戳与分配 ID。
pub trait PdClient: Send + Sync {
    fn get_ts(&self) -> Result<(i64, i64), String>;
    fn alloc_id(&self) -> Result<u64, String>;
}

impl PdClient for MockPd {
    fn get_ts(&self) -> Result<(i64, i64), String> {
        Ok(MockPd::get_ts(self))
    }

    fn alloc_id(&self) -> Result<u64, String> {
        Ok(MockPd::alloc_id(self))
    }
}

impl From<RegionError> for ServerError {
    fn from(value: RegionError) -> Self {
        Self::Region(value)
    }
}

/// ValueLog 写入缓冲选项（对齐 Badger ValueLogWriteOptions）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValueLogWriteOptions {
    pub write_buffer_size: usize,
}

/// SST/Table 构建选项：表大小、各级压缩、SURF 起始层。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableBuilderOptions {
    pub max_table_size: i64,
    pub compression_per_level: Vec<CompressionType>,
    pub surf_start_level: i32,
}

/// 打开 Database 所需的完整引擎选项快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseOptions {
    pub num_compactors: i32,
    pub value_threshold: i32,
    pub managed_transactions: bool,
    pub value_log_write_options: ValueLogWriteOptions,
    pub directory: PathBuf,
    pub value_directory: PathBuf,
    pub value_log_file_size: i64,
    pub value_log_max_files: i32,
    pub max_mem_table_size: i64,
    pub table_builder_options: TableBuilderOptions,
    pub num_mem_tables: i32,
    pub num_level_zero_tables: i32,
    pub num_level_zero_tables_stall: i32,
    pub level_one_size: i64,
    pub sync_writes: bool,
    pub max_block_cache_size: i64,
    pub max_index_cache_size: i64,
    pub compaction_filter_enabled: bool,
    pub compact_l0_when_close: bool,
    pub volatile_mode: bool,
}

/// 引擎句柄：保存打开选项与死锁检测相关标志。
pub struct Database {
    options: DatabaseOptions,
    closed: AtomicBool,
    deadlock_leader: AtomicBool,
    deadlock_detection_started: AtomicBool,
}

impl Database {
    /// 打开数据库：非 volatile 模式下创建目录。
    fn open(options: DatabaseOptions) -> Result<Self, ServerError> {
        if !options.volatile_mode {
            fs::create_dir_all(&options.directory)
                .map_err(|error| ServerError::Io(error.to_string()))?;
            if options.value_directory != options.directory {
                fs::create_dir_all(&options.value_directory)
                    .map_err(|error| ServerError::Io(error.to_string()))?;
            }
        }
        Ok(Self {
            options,
            closed: AtomicBool::new(false),
            deadlock_leader: AtomicBool::new(false),
            deadlock_detection_started: AtomicBool::new(false),
        })
    }

    /// 返回打开时使用的选项引用。
    pub fn options(&self) -> &DatabaseOptions {
        &self.options
    }

    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// 本节点是否为死锁检测 leader。
    pub fn is_deadlock_leader(&self) -> bool {
        self.deadlock_leader.load(Ordering::Acquire)
    }

    /// 死锁检测是否已启动。
    pub fn is_deadlock_detection_started(&self) -> bool {
        self.deadlock_detection_started.load(Ordering::Acquire)
    }
}

/// 引擎捆绑：Database + 内存锁存储 + 状态时间戳。
pub struct EngineBundle {
    pub database: Arc<Database>,
    pub lock_store: Arc<MemStore>,
    pub state_ts: u64,
}

impl DatabaseBundle for EngineBundle {
    fn close(&self) -> Result<(), String> {
        self.database.closed.store(true, Ordering::Release);
        Ok(())
    }
}

/// 按子路径与 Engine 配置打开 Database；raft 子路径直接返回 Unsupported。
pub fn create_db(
    sub_path: &str,
    safe_point: Option<&SafePoint>,
    config: &Engine,
) -> Result<Arc<Database>, ServerError> {
    if sub_path == SUB_PATH_RAFT {
        return Err(ServerError::Unsupported(SUB_PATH_RAFT.to_owned()));
    }
    if config.Compression.len() < BADGER_LEVEL_COUNT {
        return Err(ServerError::InvalidConfig(format!(
            "compression needs {BADGER_LEVEL_COUNT} levels, got {}",
            config.Compression.len()
        )));
    }
    // Go allocates the result with len(conf.Compression), then fills only the
    // seven levels present in Badger's default options. Keep any additional
    // levels as None instead of truncating the caller's option shape.
    let mut compression_per_level = vec![CompressionType::None; config.Compression.len()];
    for (index, value) in config
        .Compression
        .iter()
        .take(BADGER_LEVEL_COUNT)
        .enumerate()
    {
        compression_per_level[index] = ParseCompression(value);
    }
    let directory = PathBuf::from(&config.DBPath).join(sub_path);
    // 将 Engine 配置映射为 DatabaseOptions；有 safe_point 时开启 compaction filter。
    Database::open(DatabaseOptions {
        num_compactors: config.NumCompactors,
        value_threshold: config.ValueThreshold,
        managed_transactions: true,
        value_log_write_options: ValueLogWriteOptions {
            write_buffer_size: 4 * 1024 * 1024,
        },
        value_directory: directory.clone(),
        directory,
        value_log_file_size: config.VlogFileSize,
        value_log_max_files: 3,
        max_mem_table_size: config.MaxMemTableSize,
        table_builder_options: TableBuilderOptions {
            max_table_size: config.MaxTableSize,
            compression_per_level,
            surf_start_level: config.SurfStartLevel,
        },
        num_mem_tables: config.NumMemTables,
        num_level_zero_tables: config.NumL0Tables,
        num_level_zero_tables_stall: config.NumL0TablesStall,
        level_one_size: config.L1Size,
        sync_writes: config.SyncWrite,
        max_block_cache_size: config.BlockCacheSize,
        max_index_cache_size: config.IndexCacheSize,
        compaction_filter_enabled: safe_point.is_some(),
        compact_l0_when_close: config.CompactL0WhenClose,
        volatile_mode: config.VolatileMode,
    })
    .map(Arc::new)
}

/// 从顶层 Config 提取 RegionOptions（Store/PD 地址与 Region 大小）。
pub fn get_region_options(config: &Config) -> RegionOptions {
    RegionOptions {
        store_address: config.Server.StoreAddr.clone(),
        pd_address: config.Server.PDAddr.clone(),
        region_size: config.Server.RegionSize,
    }
}

/// 构造初始 Store 与覆盖全键空间的单 Region（conf_ver/version=1）。
fn initial_metadata(
    store_id: u64,
    region_id: u64,
    peer_id: u64,
    address: String,
) -> (Store, Region) {
    let store = Store {
        id: store_id,
        address,
        labels: Vec::new(),
    };
    let region = Region {
        id: region_id,
        start_key: Vec::new(),
        end_key: Vec::new(),
        epoch: RegionEpoch {
            conf_ver: 1,
            version: 1,
        },
        peers: vec![Peer {
            id: peer_id,
            store_id,
        }],
    };
    (store, region)
}

/// 将 MockRegionManager 适配为 RegionManager，并按 Region 缓存 RegionContext。
struct MockRegionAdapter {
    manager: Arc<MockRegionManager>,
    latches: Arc<Latches>,
    contexts: RwLock<HashMap<u64, Arc<RegionContext>>>,
}

impl MockRegionAdapter {
    fn new(manager: Arc<MockRegionManager>) -> Self {
        Self {
            manager,
            latches: Arc::new(Latches::default()),
            contexts: RwLock::new(HashMap::new()),
        }
    }

    /// 懒创建并缓存 RegionContext；同 ID 复用同一 latch 组。
    fn context(&self, region: Region) -> Arc<RegionContext> {
        if let Some(context) = self.contexts.read().unwrap().get(&region.id) {
            return Arc::clone(context);
        }
        let mut contexts = self.contexts.write().unwrap();
        Arc::clone(
            contexts
                .entry(region.id)
                .or_insert_with(|| Arc::new(RegionContext::new(region, Arc::clone(&self.latches)))),
        )
    }
}

impl RegionManager for MockRegionAdapter {
    fn get_region_from_context(
        &self,
        context: &RequestContext,
    ) -> Result<Arc<RegionContext>, RegionError> {
        let region = self.manager.validate_context(
            context.region_id,
            context.store_id,
            context.epoch.as_ref(),
        )?;
        Ok(self.context(region.meta))
    }

    fn get_store_info(&self, context: &RequestContext) -> Result<(String, u64), RegionError> {
        let stores = self.manager.all_stores();
        let store = match context.store_id {
            Some(id) => stores.into_iter().find(|store| store.id == id),
            None => stores.into_iter().next(),
        }
        .ok_or(RegionError::StoreNotMatch)?;
        Ok((store.address, store.id))
    }

    fn get_store_id_by_address(&self, address: &str) -> Result<u64, RegionError> {
        self.manager.get_store_id_by_addr(address)
    }

    fn get_store_address_by_id(&self, store_id: u64) -> Result<String, RegionError> {
        self.manager.get_store_addr_by_id(store_id)
    }

    fn split_region(&self, region_id: u64, keys: Vec<Vec<u8>>) -> Result<Vec<Region>, RegionError> {
        if self.manager.get_region(region_id).is_none() {
            return Err(RegionError::RegionNotFound(region_id));
        }
        let regions = self.manager.split_keys(keys)?;
        // 分裂后失效相关 RegionContext 缓存。
        let changed = regions.iter().map(|region| region.id).collect::<Vec<_>>();
        self.contexts
            .write()
            .unwrap()
            .retain(|id, _| !changed.contains(id) && *id != region_id);
        Ok(regions)
    }

    fn close(&self) {
        self.manager.close();
    }
}

/// 组装 StandAlone InnerServer + MvccStore + Server；启动前标记死锁 leader。
fn setup_stand_alone_inner_server(
    bundle: Arc<EngineBundle>,
    safe_point: Arc<SafePoint>,
    region_manager: Arc<dyn RegionManager>,
) -> Result<Arc<Server>, ServerError> {
    let inner_server = Arc::new(StandAloneInnerServer::new(Arc::clone(&bundle)));
    inner_server.setup();
    let store = Arc::new(MvccStore::new(safe_point));

    // Standalone mode owns the local wait-for graph and therefore becomes leader
    // before the inner service starts, exactly like the Go implementation.
    bundle
        .database
        .deadlock_leader
        .store(true, Ordering::Release);
    inner_server.start().map_err(ServerError::Start)?;
    bundle
        .database
        .deadlock_detection_started
        .store(true, Ordering::Release);

    Ok(Arc::new(Server::new(region_manager, store, inner_server)))
}

/// 创建带 MockPd 的模拟 Server：引导单 Store/Region，返回 Server、RegionManager、Pd。
pub fn new_mock(
    config: &Config,
    cluster_id: u64,
) -> Result<(Arc<Server>, Arc<MockRegionManager>, Arc<MockPd>), ServerError> {
    let (physical, logical) = astersql_store_mockstore_unistore_tikv::mock_region::get_ts();
    // PD 时间戳打包：physical 左移 18 位加上 logical。
    let state_ts = ((physical as u64) << 18).wrapping_add(logical as u64);
    let safe_point = Arc::new(SafePoint::new(0));
    let database = create_db(SUB_PATH_KV, Some(&safe_point), &config.Engine)?;
    let bundle = Arc::new(EngineBundle {
        database,
        lock_store: Arc::from(MemStore::NewMemStore(LOCK_STORE_ARENA_SIZE)),
        state_ts,
    });

    let region_manager = Arc::new(MockRegionManager::new(cluster_id, config.Server.RegionSize));
    let ids = region_manager.alloc_ids(3);
    let (store, region) = initial_metadata(ids[0], ids[1], ids[2], config.Server.StoreAddr.clone());
    region_manager.bootstrap(vec![store], region)?;
    let pd_client = Arc::new(MockPd::new(Arc::clone(&region_manager)));
    let adapter: Arc<dyn RegionManager> =
        Arc::new(MockRegionAdapter::new(Arc::clone(&region_manager)));
    let server = setup_stand_alone_inner_server(bundle, safe_point, adapter)?;
    Ok((server, region_manager, pd_client))
}

/// 使用外部 PdClient 创建 StandAlone Server；开启 Raft 时返回 Unsupported。
pub fn new(config: &Config, pd_client: Arc<dyn PdClient>) -> Result<Arc<Server>, ServerError> {
    let (physical, logical) = pd_client
        .get_ts()
        .map_err(|error| ServerError::Pd(error.to_string()))?;
    let state_ts = ((physical as u64) << 18).wrapping_add(logical as u64);
    let safe_point = Arc::new(SafePoint::new(0));
    let database = create_db(SUB_PATH_KV, Some(&safe_point), &config.Engine)?;
    let bundle = Arc::new(EngineBundle {
        database,
        lock_store: Arc::from(MemStore::NewMemStore(LOCK_STORE_ARENA_SIZE)),
        state_ts,
    });
    if config.Server.Raft {
        return Err(ServerError::Unsupported("raftstore".to_owned()));
    }

    // 从 PD 分配 store/region/peer 三个 ID。
    let ids = (0..3)
        .map(|_| {
            pd_client
                .alloc_id()
                .map_err(|error| ServerError::Pd(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (store, region) = initial_metadata(ids[0], ids[1], ids[2], config.Server.StoreAddr.clone());
    let region_manager: Arc<dyn RegionManager> = Arc::new(StandAloneRegionManager::new(
        store,
        region,
        get_region_options(config),
    ));
    setup_stand_alone_inner_server(bundle, safe_point, region_manager)
}
