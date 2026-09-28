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

// 统计信息同步加载（Stats Sync Load）核心实现。
//
// 查询计划需要某列/索引直方图但缓存未命中时，将加载任务入队，
// 由 SubLoadWorker 从存储高优先级读取 Histogram/CMSketch/TopN，写回缓存。
// 同 key 请求经 singleflight 合并，避免重复 IO；支持超时、重试与 panic 隔离。

use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// 单任务允许的最大重试次数（失败后可再试 RetryCount 次）。
pub const RetryCount: i32 = 1;

/// 按 CPU 核数给出建议的同步加载并发度（5–10）。
pub fn GetSyncLoadConcurrencyByCPU() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    match cores {
        0..=8 => 5,
        9..=16 => 6,
        17..=32 => 8,
        _ => 10,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 同步加载错误：通道关闭/满、超时、panic、元数据缺失等。
pub enum Error {
    ChannelClosed(&'static str),
    ChannelFull,
    Exit,
    HistogramMetaNotFound,
    InvalidData(String),
    Load(String),
    Panic(String),
    Poisoned,
    Timeout,
}

/// 将错误变体格式化为可读消息。
impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChannelClosed(name) => {
                write!(f, "cannot read from {name}, maybe the channel is closed")
            }
            Self::ChannelFull => f.write_str("Channel is full and timeout writing to channel"),
            Self::Exit => f.write_str("Stop loading since domain is closed"),
            Self::HistogramMetaNotFound => f.write_str("fail to get hist meta"),
            Self::InvalidData(message) | Self::Load(message) | Self::Panic(message) => {
                f.write_str(message)
            }
            Self::Poisoned => f.write_str("synchronous statistics state is poisoned"),
            Self::Timeout => f.write_str("sync load stats timeout"),
        }
    }
}

impl std::error::Error for Error {}
/// 本模块统一 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// 标识一张表上的列或索引统计项（表 ID + 列/索引 ID + 是否索引）。
pub struct TableItemID {
    pub TableID: i64,
    pub ID: i64,
    pub IsIndex: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一次加载请求：目标项及是否需要 FullLoad（含桶与草图）。
pub struct StatsLoadItem {
    pub TableItemID: TableItemID,
    pub FullLoad: bool,
}

impl StatsLoadItem {
    /// singleflight / 去重用的稳定键。
    pub fn Key(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.TableItemID.TableID, self.TableItemID.ID, self.TableItemID.IsIndex, self.FullLoad
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 列元信息：ID、字段类型、是否主键。
pub struct ColumnInfo {
    pub id: i64,
    pub field_type: String,
    pub primary_key: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引元信息（当前仅 ID）。
pub struct IndexInfo {
    pub id: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 轻量直方图：NDV、空值、列大小、相关位与桶边界。
/// NDV：不重复值估计；相关位用于编码 correlation。
pub struct Histogram {
    pub ndv: i64,
    pub null_count: i64,
    pub total_column_size: i64,
    pub correlation_bits: u64,
    pub last_update_version: u64,
    pub buckets: Vec<(Vec<u8>, Vec<u8>, u64)>,
}

impl Histogram {
    /// 是否已有可用统计（NDV/空值/桶任一非空）。
    fn stats_available(&self) -> bool {
        self.ndv > 0 || self.null_count > 0 || !self.buckets.is_empty()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Count-Min Sketch：二维计数矩阵，近似估计任意值频率。
pub struct CmsSketch {
    pub rows: Vec<Vec<u64>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// TopN 高频值列表：(编码值, 计数)。
pub struct TopN {
    pub values: Vec<(Vec<u8>, u64)>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 缓存项加载状态：Evicted 表示仅元数据或已驱逐；Full 表示完整加载。
pub enum LoadedStatus {
    #[default]
    Evicted,
    Full,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 缓存中的列统计对象。
pub struct Column {
    pub physical_id: i64,
    pub histogram: Histogram,
    pub info: ColumnInfo,
    pub cms: Option<CmsSketch>,
    pub top_n: Option<TopN>,
    pub is_handle: bool,
    pub stats_version: i64,
    pub loaded_status: LoadedStatus,
}

impl Column {
    /// 构造空列统计占位（未 ANALYZE 时写入缓存避免反复加载）。
    pub fn Empty(table_id: i64, is_handle: bool, info: ColumnInfo) -> Self {
        Self {
            physical_id: table_id,
            info,
            is_handle,
            ..Self::default()
        }
    }

    /// 是否已完整加载（含桶与草图）。
    pub fn IsFullLoad(&self) -> bool {
        self.loaded_status == LoadedStatus::Full
    }

    /// 直方图侧是否有可用数据。
    pub fn StatsAvailable(&self) -> bool {
        self.histogram.stats_available()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 缓存中的索引统计对象。
pub struct Index {
    pub physical_id: i64,
    pub histogram: Histogram,
    pub info: IndexInfo,
    pub cms: Option<CmsSketch>,
    pub top_n: Option<TopN>,
    pub stats_version: i64,
    pub loaded_status: LoadedStatus,
}

impl Index {
    /// 索引是否已完整加载。
    pub fn IsFullLoad(&self) -> bool {
        self.loaded_status == LoadedStatus::Full
    }

    /// stats_version 非 0 视为已 ANALYZE。
    pub fn IsAnalyzed(&self) -> bool {
        self.stats_version != 0
    }
}

#[derive(Clone, Debug, Default)]
/// 表级统计缓存：列/索引映射、已分析列集合与存在性标记。
pub struct TableStats {
    pub columns: HashMap<i64, Column>,
    pub indices: HashMap<i64, Index>,
    pub analyzed_columns: HashSet<i64>,
    pub column_exists: HashMap<i64, bool>,
    pub index_exists: HashMap<i64, bool>,
    pub stats_version: i32,
}

impl TableStats {
    /// 判断列是否需要加载：返回 (已有列, 是否仍需加载, 是否已分析)。
    pub fn ColumnIsLoadNeeded(&self, id: i64, full_load: bool) -> (Option<Column>, bool, bool) {
        let column = self.columns.get(&id).cloned();
        let needed = column
            .as_ref()
            .is_none_or(|column| full_load && !column.IsFullLoad());
        (column, needed, self.analyzed_columns.contains(&id))
    }

    /// 判断索引是否需要加载：返回 (已有索引, 是否仍需加载)。
    pub fn IndexIsLoadNeeded(&self, id: i64) -> (Option<Index>, bool) {
        let index = self.indices.get(&id).cloned();
        let needed = index.as_ref().is_none_or(|index| !index.IsFullLoad());
        (index, needed)
    }
}

#[derive(Clone, Debug, Default)]
/// 表结构摘要：是否 handle 主键、列/索引信息映射。
pub struct TableInfo {
    pub pk_is_handle: bool,
    pub columns: HashMap<i64, ColumnInfo>,
    pub indices: HashMap<i64, IndexInfo>,
}

/// 从持久化存储高优先级读取直方图元数据、完整直方图与 CMSketch/TopN。
pub trait StatsStorage: Send + Sync {
    /// 读取直方图元数据（不含桶细节），返回 (Histogram, stats_version)。
    fn HistMetaFromStorageWithHighPriority(
        &self,
        item: TableItemID,
        column_info: Option<&ColumnInfo>,
    ) -> Result<Option<(Histogram, i64)>>;

    /// 在元数据基础上加载完整桶。
    fn HistogramFromStorageWithHighPriority(
        &self,
        item: TableItemID,
        column_info: Option<&ColumnInfo>,
        metadata: &Histogram,
    ) -> Result<Histogram>;

    /// 加载 CMSketch 与 TopN。
    fn CMSketchAndTopNFromStorageWithHighPriority(
        &self,
        item: TableItemID,
        stats_version: i64,
    ) -> Result<(Option<CmsSketch>, Option<TopN>)>;
}

/// Handle 侧依赖：读缓存、表信息、更新缓存与访问 StatsStorage。
pub trait StatsHandle: Send + Sync {
    /// 获取表统计缓存快照。
    fn Get(&self, table_id: i64) -> Option<TableStats>;
    /// 按表 ID 取表结构信息。
    fn TableInfoByID(&self, table_id: i64) -> Option<TableInfo>;
    /// 写回更新后的表统计缓存。
    fn UpdateStatsCache(&self, table_id: i64, table: TableStats) -> Result<()>;
    /// 返回底层存储访问器。
    fn Storage(&self) -> &dyn StatsStorage;

    /// ANALYZE 应跳过的列类型集合（默认空）。
    fn AnalyzeSkipColumnTypes(&self) -> Result<HashSet<String>> {
        Ok(HashSet::new())
    }

    /// 统计租约时长，影响 worker 退避间隔。
    fn Lease(&self) -> Duration {
        Duration::from_secs(1)
    }
}

#[derive(Clone, Debug)]
/// 单个加载任务完成后通知 waiter 的结果。
pub struct StatsLoadResult {
    pub Item: TableItemID,
    pub Error: Option<String>,
}

impl StatsLoadResult {
    /// 是否携带错误。
    pub fn HasError(&self) -> bool {
        self.Error.is_some()
    }

    /// 错误消息；无错误时为空串。
    pub fn ErrorMsg(&self) -> &str {
        self.Error.as_deref().unwrap_or("")
    }
}

#[derive(Debug)]
/// 队列中的加载任务：目标项、超时截止与重试计数。
pub struct NeededItemTask {
    pub Item: StatsLoadItem,
    pub ToTimeout: Instant,
    pub ResultCh: mpsc::SyncSender<StatsLoadResult>,
    pub Retry: i32,
}

#[derive(Debug)]
/// 语句级同步加载上下文：超时、待加载项、结果通道与错误收集。
pub struct StatementStatsLoad {
    pub Timeout: Duration,
    pub NeededItems: Vec<StatsLoadItem>,
    result_channels: Vec<mpsc::Receiver<StatsLoadResult>>,
    pub LoadStartTime: Instant,
    pub ErrorMessages: Vec<String>,
}

impl StatementStatsLoad {
    /// 暴露与 Go `StatsLoad.ResultCh` 对应的接收端，供等待方收集结果。
    /// Exposes Go `StatsLoad.ResultCh` receivers for sync-load waiters.
    pub fn ResultCh(&mut self) -> &mut Vec<mpsc::Receiver<StatsLoadResult>> {
        &mut self.result_channels
    }
}

/// 默认空上下文；Timeout 为 0，LoadStartTime 为 now。
impl Default for StatementStatsLoad {
    fn default() -> Self {
        Self {
            Timeout: Duration::ZERO,
            NeededItems: Vec::new(),
            result_channels: Vec::new(),
            LoadStartTime: Instant::now(),
            ErrorMessages: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
/// 语句上下文中的统计加载子结构（精简版）。
pub struct StatementContext {
    pub StatsLoad: StatementStatsLoad,
}

/// singleflight：同一 key 下挂起的多个结果发送端。
type WaiterMap = HashMap<String, Vec<mpsc::SyncSender<StatsLoadResult>>>;

/// 同步加载调度器：双队列（needed/timeout）、singleflight 与指标计数。
pub struct statsSyncLoad {
    stats_handle: Arc<dyn StatsHandle>,
    needed_items_sender: mpsc::SyncSender<NeededItemTask>,
    needed_items_receiver: Mutex<mpsc::Receiver<NeededItemTask>>,
    timeout_items_sender: mpsc::SyncSender<NeededItemTask>,
    timeout_items_receiver: Mutex<mpsc::Receiver<NeededItemTask>>,
    mutex_for_stats_cache: Mutex<()>,
    singleflight: Arc<Mutex<WaiterMap>>,
    sync_load_count: AtomicU64,
    sync_load_timeout_count: AtomicU64,
    sync_load_dedup_count: Arc<AtomicU64>,
}

/// 创建有界同步通道的 statsSyncLoad。
pub fn NewStatsSyncLoad(stats_handle: Arc<dyn StatsHandle>, queue_size: usize) -> statsSyncLoad {
    let (needed_items_sender, needed_items_receiver) = mpsc::sync_channel(queue_size);
    let (timeout_items_sender, timeout_items_receiver) = mpsc::sync_channel(queue_size);
    statsSyncLoad {
        stats_handle,
        needed_items_sender,
        needed_items_receiver: Mutex::new(needed_items_receiver),
        timeout_items_sender,
        timeout_items_receiver: Mutex::new(timeout_items_receiver),
        mutex_for_stats_cache: Mutex::new(()),
        singleflight: Arc::new(Mutex::new(HashMap::new())),
        sync_load_count: AtomicU64::new(0),
        sync_load_timeout_count: AtomicU64::new(0),
        sync_load_dedup_count: Arc::new(AtomicU64::new(0)),
    }
}

impl statsSyncLoad {
    /// 过滤已加载项后，为每个剩余项注册 singleflight 并尝试入队；向语句上下文挂结果接收端。
    pub fn SendLoadRequests(
        &self,
        statement_context: &mut StatementContext,
        needed_histogram_items: &[StatsLoadItem],
        timeout: Duration,
    ) -> Result<()> {
        // 跳过缓存中已满足 FullLoad/元数据需求的列与索引。
        let remained_items = self.removeHistLoadedColumns(needed_histogram_items);
        if remained_items.is_empty() {
            return Ok(());
        }
        statement_context.StatsLoad.Timeout = timeout;
        statement_context.StatsLoad.NeededItems = remained_items.clone();
        statement_context.StatsLoad.result_channels.clear();
        for item in remained_items {
            let (result_sender, result_receiver) = mpsc::sync_channel(1);
            let singleflight_key = item.Key();
            // 首个同 key 请求成为 leader 负责入队；其余只挂 waiter。
            let leader = {
                let mut groups = self.singleflight.lock().map_err(|_| Error::Poisoned)?;
                match groups.get_mut(&singleflight_key) {
                    Some(waiters) => {
                        waiters.push(result_sender);
                        false
                    }
                    None => {
                        groups.insert(singleflight_key.clone(), vec![result_sender]);
                        true
                    }
                }
            };
            if leader {
                let sender = self.needed_items_sender.clone();
                let groups = Arc::clone(&self.singleflight);
                let dedup_count = Arc::clone(&self.sync_load_dedup_count);
                let leader_key = singleflight_key.clone();
                std::thread::spawn(move || {
                    let deadline = Instant::now() + timeout;
                    let item_id = item.TableItemID;
                    let (task_result_sender, task_result_receiver) = mpsc::sync_channel(1);
                    let mut task = NeededItemTask {
                        Item: item,
                        ToTimeout: deadline,
                        ResultCh: task_result_sender,
                        Retry: 0,
                    };
                    // 与 Go singleflight 函数一致：先在时限内入队，再在同一时限内
                    // 等待 worker 结果；任一阶段超时都完成并释放整个 waiter 组。
                    loop {
                        match sender.try_send(task) {
                            Ok(()) => {
                                dedup_count.fetch_add(1, Ordering::Relaxed);
                                let remaining = deadline.saturating_duration_since(Instant::now());
                                let result = if remaining.is_zero() {
                                    StatsLoadResult {
                                        Item: item_id,
                                        Error: Some("sync load took too long to return".into()),
                                    }
                                } else {
                                    match task_result_receiver.recv_timeout(remaining) {
                                        Ok(result) => result,
                                        Err(mpsc::RecvTimeoutError::Timeout) => StatsLoadResult {
                                            Item: item_id,
                                            Error: Some("sync load took too long to return".into()),
                                        },
                                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                                            StatsLoadResult {
                                                Item: item_id,
                                                Error: Some(
                                                    "sync load stats channel closed unexpectedly"
                                                        .into(),
                                                ),
                                            }
                                        }
                                    }
                                };
                                complete_waiters(&groups, &leader_key, result);
                                break;
                            }
                            Err(mpsc::TrySendError::Full(returned)) => {
                                task = returned;
                                if Instant::now() >= deadline {
                                    complete_waiters(
                                        &groups,
                                        &leader_key,
                                        StatsLoadResult {
                                            Item: item_id,
                                            Error: Some("sync load stats channel is full and timeout sending task to channel".into()),
                                        },
                                    );
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            Err(mpsc::TrySendError::Disconnected(returned)) => {
                                complete_waiters(
                                    &groups,
                                    &leader_key,
                                    StatsLoadResult {
                                        Item: item_id,
                                        Error: Some(
                                            "sync load stats channel closed unexpectedly".into(),
                                        ),
                                    },
                                );
                                break;
                            }
                        }
                    }
                });
            }
            statement_context
                .StatsLoad
                .result_channels
                .push(result_receiver);
        }
        statement_context.StatsLoad.LoadStartTime = Instant::now();
        Ok(())
    }

    /// 阻塞等待本语句所有加载结果；超时或通道断开则返回错误。
    pub fn SyncWaitStatsLoad(&self, statement_context: &mut StatementContext) -> Result<()> {
        if statement_context.StatsLoad.NeededItems.is_empty() {
            return Ok(());
        }
        statement_context.StatsLoad.ErrorMessages.clear();
        let mut unchecked: HashSet<_> = statement_context
            .StatsLoad
            .NeededItems
            .iter()
            .map(|item| item.TableItemID)
            .collect();
        let deadline = Instant::now() + statement_context.StatsLoad.Timeout;
        for receiver in statement_context.StatsLoad.result_channels.drain(..) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.sync_load_timeout_count.fetch_add(1, Ordering::Relaxed);
                statement_context.StatsLoad.NeededItems.clear();
                return Err(Error::Timeout);
            }
            self.sync_load_count.fetch_add(1, Ordering::Relaxed);
            match receiver.recv_timeout(remaining) {
                Ok(result) => {
                    if let Some(error) = result.Error {
                        statement_context.StatsLoad.ErrorMessages.push(error);
                    }
                    unchecked.remove(&result.Item);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.sync_load_timeout_count.fetch_add(1, Ordering::Relaxed);
                    statement_context.StatsLoad.NeededItems.clear();
                    return Err(Error::Timeout);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    statement_context.StatsLoad.NeededItems.clear();
                    return Err(Error::ChannelClosed("sync load stats channel"));
                }
            }
        }
        statement_context.StatsLoad.NeededItems.clear();
        let _all_results_returned = unchecked.is_empty();
        Ok(())
    }

    /// 过滤掉缓存中已不需要再加载的项。
    pub fn removeHistLoadedColumns(&self, needed_items: &[StatsLoadItem]) -> Vec<StatsLoadItem> {
        needed_items
            .iter()
            .filter_map(|item| {
                let table = self.stats_handle.Get(item.TableItemID.TableID)?;
                let needed = if item.TableItemID.IsIndex {
                    table.IndexIsLoadNeeded(item.TableItemID.ID).1
                } else {
                    table
                        .ColumnIsLoadNeeded(item.TableItemID.ID, item.FullLoad)
                        .1
                };
                needed.then(|| item.clone())
            })
            .collect()
    }

    /// 在超时内将任务写入 needed 通道；满则重试，断开则报错。
    pub fn AppendNeededItem(&self, mut task: NeededItemTask, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.needed_items_sender.try_send(task) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Full(returned)) => {
                    task = returned;
                    if Instant::now() >= deadline {
                        return Err(Error::ChannelFull);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err(Error::ChannelClosed("NeededItemsCh"));
                }
            }
        }
    }

    /// Worker 主循环：反复 HandleOneTask，失败或有残留任务时按租约退避。
    pub fn SubLoadWorker(&self, exit: &AtomicBool) {
        let mut last_task = None;
        while !exit.load(Ordering::Acquire) {
            match self.HandleOneTask(last_task, exit) {
                Ok(task) => {
                    if task.is_some() {
                        let jitter = Instant::now().elapsed().subsec_nanos() as u64 % 500;
                        std::thread::sleep(
                            self.stats_handle.Lease() / 10 + Duration::from_micros(jitter),
                        );
                    }
                    last_task = task;
                }
                Err(Error::Exit) => return,
                Err(_) => {
                    let jitter = Instant::now().elapsed().subsec_nanos() as u64 % 500;
                    std::thread::sleep(
                        self.stats_handle.Lease() / 10 + Duration::from_micros(jitter),
                    );
                    last_task = None;
                }
            }
        }
    }

    /// 处理单个任务：成功则通知 waiter；可重试则返回任务；否则带错完成。
    /// 用 catch_unwind 隔离加载路径 panic。
    pub fn HandleOneTask(
        &self,
        last_task: Option<NeededItemTask>,
        exit: &AtomicBool,
    ) -> Result<Option<NeededItemTask>> {
        let mut task = match last_task {
            Some(task) => task,
            None => self.drainColTask(exit)?,
        };
        let load_result = match catch_unwind(AssertUnwindSafe(|| self.handleOneItemTask(&task))) {
            Ok(result) => result,
            Err(panic) => Err(Error::Panic(panic_message(panic))),
        };
        match load_result {
            Ok(()) => {
                self.finish_task(&task, None);
                Ok(None)
            }
            Err(_) if isVaildForRetry(&mut task) => Ok(Some(task)),
            Err(error) => {
                self.finish_task(&task, Some(error.to_string()));
                Ok(None)
            }
        }
    }

    /// 将任务结果交给 singleflight leader，由 leader 广播给所有 waiter。
    fn finish_task(&self, task: &NeededItemTask, error: Option<String>) {
        let _ = task.ResultCh.send(StatsLoadResult {
            Item: task.Item.TableItemID,
            Error: error,
        });
    }

    /// 加载入口，转发到带语句上下文的实现。
    fn handleOneItemTask(&self, task: &NeededItemTask) -> Result<()> {
        self.handleOneItemTaskWithSCtx(task)
    }

    /// 判断是否需加载、跳过类型、读存储并更新缓存。
    fn handleOneItemTaskWithSCtx(&self, task: &NeededItemTask) -> Result<()> {
        let skip_types = self
            .stats_handle
            .AnalyzeSkipColumnTypes()
            .unwrap_or_default();
        let item = task.Item.TableItemID;
        let Some(table_stats) = self.stats_handle.Get(item.TableID) else {
            return Ok(());
        };
        let Some(table_info) = self.stats_handle.TableInfoByID(item.TableID) else {
            return Ok(());
        };
        let mut wrapper = StatsWrapper::default();
        if item.IsIndex {
            let (index, load_needed) = table_stats.IndexIsLoadNeeded(item.ID);
            if !load_needed {
                return Ok(());
            }
            wrapper.index_info = index
                .map(|index| index.info)
                .or_else(|| table_info.indices.get(&item.ID).cloned());
        } else {
            let (column, load_needed, analyzed) =
                table_stats.ColumnIsLoadNeeded(item.ID, task.Item.FullLoad);
            if !load_needed {
                return Ok(());
            }
            wrapper.column_info = column
                .map(|column| column.info)
                .or_else(|| table_info.columns.get(&item.ID).cloned());
            let Some(info) = wrapper.column_info.as_ref() else {
                return Ok(());
            };
            if skip_types.contains(&info.field_type) {
                return Ok(());
            }
            // 未 ANALYZE：写入 Empty 占位后返回，避免反复打存储。
            if !analyzed {
                wrapper.column = Some(Column::Empty(
                    item.TableID,
                    table_info.pk_is_handle && info.primary_key,
                    info.clone(),
                ));
                self.updateCachedItem(item, wrapper.column.take(), None, task.Item.FullLoad)?;
                return Ok(());
            }
        }
        let wrapper = match self.readStatsForOneItem(
            item,
            wrapper,
            table_info.pk_is_handle,
            task.Item.FullLoad,
        ) {
            Err(Error::HistogramMetaNotFound) => return Ok(()),
            result => result?,
        };
        let needs_update = if item.IsIndex {
            wrapper.index_info.is_some()
        } else {
            wrapper.column_info.is_some()
        };
        if needs_update {
            self.updateCachedItem(item, wrapper.column, wrapper.index, task.Item.FullLoad)?;
        }
        Ok(())
    }

    /// 从存储组装 Column/Index：元数据必读；FullLoad 时再读桶与草图。
    fn readStatsForOneItem(
        &self,
        item: TableItemID,
        mut wrapper: StatsWrapper,
        is_pk_is_handle: bool,
        full_load: bool,
    ) -> Result<StatsWrapper> {
        let Some((mut histogram, stats_version)) = self
            .stats_handle
            .Storage()
            .HistMetaFromStorageWithHighPriority(item, wrapper.column_info.as_ref())?
        else {
            return Err(Error::HistogramMetaNotFound);
        };
        // 非 FullLoad 仅保留元数据级 Histogram，状态标为 Evicted。
        let (cms, top_n) = if full_load {
            histogram = self
                .stats_handle
                .Storage()
                .HistogramFromStorageWithHighPriority(
                    item,
                    wrapper.column_info.as_ref(),
                    &histogram,
                )?;
            self.stats_handle
                .Storage()
                .CMSketchAndTopNFromStorageWithHighPriority(item, stats_version)?
        } else {
            (None, None)
        };
        let status = if full_load {
            LoadedStatus::Full
        } else {
            LoadedStatus::Evicted
        };
        if item.IsIndex {
            let Some(info) = wrapper.index_info.clone() else {
                return Ok(wrapper);
            };
            wrapper.index = Some(Index {
                physical_id: item.TableID,
                histogram,
                info,
                cms,
                top_n,
                stats_version,
                loaded_status: if stats_version == 0 {
                    LoadedStatus::Evicted
                } else {
                    status
                },
            });
        } else {
            let Some(info) = wrapper.column_info.clone() else {
                return Ok(wrapper);
            };
            let available = histogram.stats_available();
            wrapper.column = Some(Column {
                physical_id: item.TableID,
                histogram,
                is_handle: is_pk_is_handle && info.primary_key,
                info,
                cms,
                top_n,
                stats_version,
                loaded_status: if available {
                    status
                } else {
                    LoadedStatus::Evicted
                },
            });
        }
        Ok(wrapper)
    }

    /// 从 needed/timeout 队列取任务；已超时的转到 timeout 通道，优先处理未超时任务。
    fn drainColTask(&self, exit: &AtomicBool) -> Result<NeededItemTask> {
        loop {
            if exit.load(Ordering::Acquire) {
                return Err(Error::Exit);
            }
            if let Some(task) = self.try_receive_needed()? {
                if Instant::now() > task.ToTimeout {
                    self.writeToTimeoutChan(task);
                    continue;
                }
                return Ok(task);
            }
            if let Some(timeout_task) = self.try_receive_timeout()? {
                if let Some(needed_task) = self.try_receive_needed()? {
                    self.writeToTimeoutChan(timeout_task);
                    return Ok(needed_task);
                }
                return Ok(timeout_task);
            }
            let result = self
                .needed_items_receiver
                .lock()
                .map_err(|_| Error::Poisoned)?
                .recv_timeout(Duration::from_millis(10));
            match result {
                Ok(task) if Instant::now() > task.ToTimeout => self.writeToTimeoutChan(task),
                Ok(task) => return Ok(task),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Error::ChannelClosed("NeededItemsCh"));
                }
            }
        }
    }

    /// 非阻塞尝试从 needed 通道取任务。
    fn try_receive_needed(&self) -> Result<Option<NeededItemTask>> {
        match self
            .needed_items_receiver
            .lock()
            .map_err(|_| Error::Poisoned)?
            .try_recv()
        {
            Ok(task) => Ok(Some(task)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(Error::ChannelClosed("NeededItemsCh")),
        }
    }

    /// 非阻塞尝试从 timeout 通道取任务。
    fn try_receive_timeout(&self) -> Result<Option<NeededItemTask>> {
        match self
            .timeout_items_receiver
            .lock()
            .map_err(|_| Error::Poisoned)?
            .try_recv()
        {
            Ok(task) => Ok(Some(task)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(Error::ChannelClosed("TimeoutItemsCh")),
        }
    }

    /// 将已超时任务尽力写入 timeout 通道（满则丢弃发送结果）。
    fn writeToTimeoutChan(&self, task: NeededItemTask) {
        let _ = self.timeout_items_sender.try_send(task);
    }

    /// 在缓存互斥下合并列/索引统计；已 FullLoad 且本次非全量则跳过覆盖。
    fn updateCachedItem(
        &self,
        item: TableItemID,
        column_histogram: Option<Column>,
        index_histogram: Option<Index>,
        full_loaded: bool,
    ) -> Result<bool> {
        let _guard = self
            .mutex_for_stats_cache
            .lock()
            .map_err(|_| Error::Poisoned)?;
        let Some(mut table) = self.stats_handle.Get(item.TableID) else {
            return Ok(false);
        };
        if !item.IsIndex {
            let Some(column) = column_histogram else {
                return Ok(false);
            };
            if table
                .columns
                .get(&item.ID)
                .is_some_and(|cached| cached.IsFullLoad() || !full_loaded)
            {
                return Ok(false);
            }
            let available = column.StatsAvailable();
            if available {
                table.column_exists.insert(item.ID, true);
            }
            if column.stats_version != 0 {
                table.stats_version = column.stats_version as i32;
            }
            table.column_exists.insert(item.ID, available);
            table.columns.insert(item.ID, column);
        } else {
            let Some(index) = index_histogram else {
                return Ok(false);
            };
            if table
                .indices
                .get(&item.ID)
                .is_some_and(|cached| cached.IsFullLoad() || !full_loaded)
            {
                return Ok(true);
            }
            if index.IsAnalyzed() {
                table.index_exists.insert(item.ID, true);
                table.stats_version = index.stats_version as i32;
            }
            table.indices.insert(item.ID, index);
        }
        self.stats_handle.UpdateStatsCache(item.TableID, table)?;
        Ok(true)
    }

    /// 返回 (加载次数, 超时次数, 去重入队次数)。
    pub fn metrics(&self) -> (u64, u64, u64) {
        (
            self.sync_load_count.load(Ordering::Relaxed),
            self.sync_load_timeout_count.load(Ordering::Relaxed),
            self.sync_load_dedup_count.load(Ordering::Relaxed),
        )
    }
}

#[derive(Default)]
/// 单次加载中间结果：列/索引 info 与组装出的 Column/Index。
struct StatsWrapper {
    column_info: Option<ColumnInfo>,
    index_info: Option<IndexInfo>,
    column: Option<Column>,
    index: Option<Index>,
}

/// 递增 Retry 并判断是否仍允许重试（命名保留 Go 拼写 isVaild）。
pub fn isVaildForRetry(task: &mut NeededItemTask) -> bool {
    task.Retry += 1;
    task.Retry <= RetryCount
}

/// 取出并通知 singleflight 下全部 waiter。
fn complete_waiters(
    groups: &Arc<Mutex<WaiterMap>>,
    singleflight_key: &str,
    result: StatsLoadResult,
) {
    let waiters = groups
        .lock()
        .ok()
        .and_then(|mut groups| groups.remove(singleflight_key))
        .unwrap_or_default();
    for waiter in waiters {
        let _ = waiter.send(result.clone());
    }
}

/// 将 catch_unwind 的 panic payload 转为错误字符串。
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("stats loading panicked: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("stats loading panicked: {message}")
    } else {
        "stats loading panicked".into()
    }
}
