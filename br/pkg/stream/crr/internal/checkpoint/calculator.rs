// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! CRR 下游安全检查点计算器：对齐 Go `br/pkg/stream/crr/internal/checkpoint/calculator.go`。
//! 核心数据流：轮询上游全局检查点 → 规划本轮待同步对象 → 等待下游对象就绪 → 推进持久化状态。
//! Calculator 有状态，期望跨轮复用；`RestorePersistentState` 仅允许在首次计算前调用。
//! 本文件定义公开类型、依赖 trait 与主循环；轮次规划/等待细节在同包其他模块。
//! Observer 不得阻塞或回写 Calculator，以免拖死轮询与污染内部状态。
//! 安全含义：只有上游检查点推进且相关对象在下游可消费后，才允许提升返回值。
//! 失败不回滚已成功发出的观测事件，调用方应以返回错误为准重试本轮。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use astersql_br_pkg_streamhelper::Store;

use crate::progress::{
    observe_calculation_failed, observe_checkpoint_advanced, observe_round_planned,
};
use crate::storage::validate_incremental_meta_scan_storage;

// 与 Go 常量一致：上游无进展时的默认休眠间隔。
pub const DefaultPollInterval: Duration = Duration::from_secs(2);
// 读取 backupmeta 的默认并发度；<=0 时 NewCalculator 回退到该值。
pub const DefaultMetaReadConcurrency: i32 = 16;

/// 检查点计算进度事件类型；字符串值与 Go EventType 常量对齐，供指标/日志消费。
/// 变体覆盖等待上游、上游推进、轮次规划、等待下游、检查点推进与计算失败。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventType {
    // 轮询中尚未看到更大的上游全局检查点。
    EventWaitingUpstream,
    // 上游全局检查点相对上次有前进。
    EventUpstreamAdvanced,
    // 已规划本轮 pending 路径与统计。
    EventRoundPlanned,
    // 正在确认下游对象同步完成。
    EventWaitingDownstream,
    // 本轮安全检查点已提交到内部状态。
    EventCheckpointAdvanced,
    // 任一步骤失败；可携带部分 FileStatistic。
    EventCalculationFailed,
}

impl Default for EventType {
    fn default() -> Self {
        // 默认落在等待上游，避免零值被误读成已推进。
        EventType::EventWaitingUpstream
    }
}

impl EventType {
    /// 稳定 wire/日志字符串，禁止随意改名以免破坏外部观测契约。
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::EventWaitingUpstream => "waiting_upstream",
            EventType::EventUpstreamAdvanced => "upstream_advanced",
            EventType::EventRoundPlanned => "round_planned",
            EventType::EventWaitingDownstream => "waiting_downstream",
            EventType::EventCheckpointAdvanced => "checkpoint_advanced",
            EventType::EventCalculationFailed => "calculation_failed",
        }
    }
}

/// 单次进度事件载荷：携带轮次快照与可选统计/错误，供 Observer 异步观测。
#[derive(Clone, Debug, Default)]
pub struct CheckpointEvent {
    pub Type: EventType,
    // 未填时 observe 会补当前时间，对齐 Go Time.IsZero 填充语义。
    pub Time: Option<SystemTime>,

    pub TaskName: String,

    pub LoopIteration: u64,
    pub UpstreamCheckpoint: u64,
    pub SyncedTS: u64,
    // store_id → 已确认同步的 flush ts；失败路径也尽量带上便于诊断。
    pub SyncedByStore: HashMap<u64, u64>,
    // Rust HashMap 没有 Go nil map 状态；此标记区分“未携带”与“显式携带空 map”。
    pub SyncedByStoreSet: bool,

    pub AliveStoreCount: i32,
    pub PendingFileCount: i32,

    pub Statistic: Option<FileStatistic>,

    pub Err: Option<Error>,
}

/// 本轮文件侧统计：上游读到的 meta、跳过数、下游检查次数与后缀分布。
#[derive(Clone, Debug, Default)]
pub struct FileStatistic {
    pub UpstreamReadMetaFileCount: i32,
    pub SkippedStoreSyncedMetaFileCount: i32,
    pub EstimatedSyncLogFileCount: i32,
    pub DownstreamCheckFileCount: i32,
    pub PlannedFileSuffixCounts: HashMap<String, i32>,
    pub DownstreamCheckFileSuffixCounts: HashMap<String, i32>,
}

/// 进度观察者；实现不得阻塞或修改 Calculator（与 Go Observer 约束一致）。
pub trait Observer: Send + Sync {
    fn OnCheckpointEvent(&self, event: CheckpointEvent);
}

/// WalkDir 选项：子目录与游标，对齐 storeapi.WalkOption 的增量扫描语义。
#[derive(Clone, Debug, Default)]
pub struct WalkOption {
    pub SubDir: String,
    pub StartAfter: String,
}

/// 上游 PD 元数据读取：全局任务检查点与存活 store 列表。
pub trait PDMetaReader: Send + Sync {
    /// 按任务名读取上游全局检查点（TS）。
    fn GetGlobalCheckpointForTask(&self, ctx: &Context, taskName: &str) -> Result<u64, Error>;
    /// 列出当前存活 store，用于过滤已下线节点上的 meta。
    fn Stores(&self, ctx: &Context) -> Result<Vec<Store>, Error>;
}

/// 上游对象存储读取：遍历目录、读文件、暴露 URI（用于增量 meta 校验）。
pub trait UpstreamStorageReader: Send + Sync {
    /// 增量遍历；callback 返回错误应中止 Walk。
    fn WalkDir(
        &self,
        ctx: &Context,
        opt: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error>;
    /// 读取单个对象内容（通常为 backupmeta）。
    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>, Error>;
    /// 存储 URI；用于校验是否支持增量 meta 扫描前缀。
    fn URI(&self) -> String;
}

/// 下游对象同步判定：文件是否已可被恢复侧安全消费。
pub trait ObjectSyncChecker: Send + Sync {
    /// true 表示可安全消费；错误应向上传播以中止本轮。
    fn FileSynced(&self, ctx: &Context, name: &str) -> Result<bool, Error>;
}

/// 纯存在性检查；常作为 ObjectSyncChecker 的最简实现底座。
pub trait FileExistenceChecker: Send + Sync {
    /// 对象是否存在于下游可见命名空间。
    fn FileExists(&self, ctx: &Context, name: &str) -> Result<bool, Error>;
}

/// 将 FileExists 适配为 FileSynced：存在即视为已同步（测试/本地场景常用）。
pub struct ExistenceSyncChecker<C: FileExistenceChecker> {
    checker: C,
}

/// 构造适配器；不在此校验 checker 非空，由调用方保证可用。
pub fn NewExistenceSyncChecker<C: FileExistenceChecker>(checker: C) -> ExistenceSyncChecker<C> {
    ExistenceSyncChecker { checker }
}

impl<C: FileExistenceChecker> ObjectSyncChecker for ExistenceSyncChecker<C> {
    fn FileSynced(&self, ctx: &Context, name: &str) -> Result<bool, Error> {
        // 存在性即同步：与 Go existenceSyncChecker 行为一致。
        self.checker.FileExists(ctx, name)
    }
}

/// 计算器运行参数；非法间隔/并发在 NewCalculator 中被默认值纠正。
#[derive(Clone, Debug)]
pub struct CheckpointCalculatorConfig {
    pub TaskName: String,
    pub PollInterval: Duration,
    pub MetaReadConcurrency: i32,
}

impl Default for CheckpointCalculatorConfig {
    fn default() -> Self {
        Self {
            TaskName: String::new(),
            PollInterval: DefaultPollInterval,
            MetaReadConcurrency: DefaultMetaReadConcurrency,
        }
    }
}

/// 可持久化进度快照：重启后用于恢复 last_checkpoint / synced_ts / 按 store 进度。
#[derive(Clone, Debug, Default)]
pub struct PersistentState {
    pub LastCheckpoint: u64,
    pub SyncedTS: u64,
    pub SyncedByStore: HashMap<u64, u64>,
}

/// 包内轻量错误类型；消息对齐 Go fmt.Errorf 文本以便 parity 对照。
#[derive(Clone, Debug)]
pub struct Error {
    msg: String,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    pub fn message(&self) -> &str {
        &self.msg
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl std::error::Error for Error {}

/// 轻量 Context：仅建模 cancel/deadline，对应 Go context 在本包中的最小子集。
#[derive(Clone)]
pub struct Context {
    cancelled: Arc<AtomicBool>,
    cancellation_ancestors: Vec<Arc<AtomicBool>>,
    deadline: Option<Instant>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            cancellation_ancestors: Vec::new(),
            deadline: None,
        }
    }
}

impl Context {
    /// 空上下文，无截止时间、未取消。
    pub fn Background() -> Self {
        Self::default()
    }

    /// 建立父子取消链；父取消传播给子，子取消不回传父。
    pub fn WithCancel(parent: &Self) -> (Self, impl Fn() + Send + Sync + 'static) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut cancellation_ancestors = parent.cancellation_ancestors.clone();
        cancellation_ancestors.push(parent.cancelled.clone());
        let ctx = Self {
            cancelled,
            cancellation_ancestors,
            deadline: parent.deadline,
        };
        let flag = ctx.cancelled.clone();
        (ctx, move || flag.store(true, Ordering::SeqCst))
    }

    /// 在父级之上叠加超时；超时与取消共用 Err()/Done() 观测路径。
    pub fn WithTimeout(
        parent: &Self,
        timeout: Duration,
    ) -> (Self, impl Fn() + Send + Sync + 'static) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut cancellation_ancestors = parent.cancellation_ancestors.clone();
        cancellation_ancestors.push(parent.cancelled.clone());
        let requested_deadline = Instant::now() + timeout;
        let ctx = Self {
            cancelled,
            cancellation_ancestors,
            deadline: Some(parent.deadline.map_or(requested_deadline, |deadline| {
                deadline.min(requested_deadline)
            })),
        };
        let flag = ctx.cancelled.clone();
        (ctx, move || flag.store(true, Ordering::SeqCst))
    }

    /// Done 等价于 Err 有值，与 Go ctx.Done 可观测语义对齐。
    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }

    /// 优先报告取消，其次截止超时；二者消息文本固定以便测试匹配。
    pub fn Err(&self) -> Option<Error> {
        if self.cancelled.load(Ordering::SeqCst)
            || self
                .cancellation_ancestors
                .iter()
                .any(|flag| flag.load(Ordering::SeqCst))
        {
            return Some(Error::new("context canceled"));
        }
        if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                return Some(Error::new("context deadline exceeded"));
            }
        }
        None
    }
}

/// 外部依赖聚合：PD、上游存储、下游同步检查；缺一不可。
pub struct CalculatorDeps {
    pub PD: Box<dyn PDMetaReader>,
    pub Upstream: Box<dyn UpstreamStorageReader>,
    pub Sync: Box<dyn ObjectSyncChecker>,
}

/// 有状态检查点计算器；跨轮复用以保留 synced 进度。
pub struct Calculator {
    pub(crate) deps: CalculatorDeps,
    pub(crate) cfg: CheckpointCalculatorConfig,
    observer: Option<Box<dyn Observer>>,
    pub(crate) state: calculatorState,
}

impl fmt::Debug for Calculator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 不打印 deps/observer，避免 trait 对象与回调噪声。
        f.debug_struct("Calculator")
            .field("cfg", &self.cfg)
            .field("state", &self.state)
            .finish()
    }
}

/// 内部可变进度；字段语义与 PersistentState 一一对应。
#[derive(Clone, Debug, Default)]
pub(crate) struct calculatorState {
    pub last_checkpoint: u64,
    pub synced_ts: u64,
    pub synced_by_store: HashMap<u64, u64>,
}

/// 创建计算器：校验依赖与任务名，纠正非法轮询/并发配置。
pub fn NewCalculator(
    deps: CalculatorDeps,
    mut cfg: CheckpointCalculatorConfig,
    observer: Option<Box<dyn Observer>>,
) -> Result<Calculator, Error> {
    deps.validate()?;
    // 任务名为空无法定位上游全局检查点。
    if cfg.TaskName.is_empty() {
        return Err(Error::new("task name must not be empty"));
    }
    // <=0 视为未配置，回退默认，对齐 Go NewCalculator。
    if cfg.PollInterval <= Duration::ZERO {
        cfg.PollInterval = DefaultPollInterval;
    }
    if cfg.MetaReadConcurrency <= 0 {
        cfg.MetaReadConcurrency = DefaultMetaReadConcurrency;
    }

    Ok(Calculator {
        deps,
        cfg,
        observer,
        // 显式空 map，避免 nil/空语义分歧。
        state: calculatorState {
            synced_by_store: HashMap::new(),
            ..Default::default()
        },
    })
}

impl Calculator {
    /// 等待上游进展、确认必要对象已同步，返回下游安全检查点。
    /// 上游未推进时返回上次检查点且不报错；失败时仍尝试发出 CalculationFailed 事件。
    pub fn ComputeNextCheckpoint(&mut self, ctx: &Context) -> Result<u64, Error> {
        let mut statistic: Option<FileStatistic> = None;

        // 用闭包集中主路径，便于统一失败观测（对应 Go defer）。
        let result = (|| {
            let (upstream_checkpoint, advanced) = self.poll_upstream_checkpoint(ctx)?;
            if !advanced {
                // 无进展：直接回传旧值，避免空转推进状态。
                return Ok(self.state.last_checkpoint);
            }

            let alive_stores = self.load_alive_stores(ctx)?;
            let mut round = self.plan_round(ctx)?;
            statistic = Some(observe_round_planned(
                self,
                upstream_checkpoint,
                &alive_stores,
                &round,
            ));
            // 等待下游同步失败时仍快照统计，供失败事件诊断。
            if let Err(err) =
                self.wait_object_sync(ctx, &mut round.pending_paths, &mut round.statistic)
            {
                statistic = Some(round.statistic.snapshot());
                return Err(err);
            }
            statistic = Some(round.statistic.snapshot());

            // 先更新按 store 同步进度，再提交 last_checkpoint。
            self.advance_synced_state(&alive_stores, &round.max_flush_ts_by_store);
            self.state.last_checkpoint = upstream_checkpoint;
            observe_checkpoint_advanced(
                self,
                upstream_checkpoint,
                &alive_stores,
                statistic.clone(),
            );
            Ok(upstream_checkpoint)
        })();

        if let Err(err) = &result {
            observe_calculation_failed(self, err.clone(), statistic);
        }
        result
    }

    /// 当前追踪的全局 synced_ts（各 store 进度的聚合结果）。
    pub fn SyncedTS(&self) -> u64 {
        self.state.synced_ts
    }

    /// 最近一次成功返回给调用方的检查点。
    pub fn LastCheckpoint(&self) -> u64 {
        self.state.last_checkpoint
    }

    /// 导出可持久化快照；map 深拷贝以防调用方改写内部状态。
    pub fn StateSnapshot(&self) -> PersistentState {
        PersistentState {
            LastCheckpoint: self.state.last_checkpoint,
            SyncedTS: self.state.synced_ts,
            SyncedByStore: self.state.synced_by_store.clone(),
        }
    }

    /// 仅允许在计算开始前恢复；已有 last_checkpoint!=0 时拒绝覆盖。
    pub fn RestorePersistentState(&mut self, state: PersistentState) -> Result<(), Error> {
        if self.state.last_checkpoint != 0 {
            return Err(Error::new(
                "cannot restore persistent state after checkpoint calculation started",
            ));
        }
        self.state.last_checkpoint = state.LastCheckpoint;
        self.state.synced_ts = state.SyncedTS;
        self.state.synced_by_store = state.SyncedByStore;
        // 空 map 归一化，避免后续写入时的 None/空分支分裂。
        if self.state.synced_by_store.is_empty() {
            self.state.synced_by_store = HashMap::new();
        }
        Ok(())
    }

    /// 投递事件；无 observer 时静默；Time 为空则填当前时刻。
    pub(crate) fn observe(&self, mut event: CheckpointEvent) {
        let Some(observer) = &self.observer else {
            return;
        };
        if event.Time.is_none() {
            event.Time = Some(SystemTime::now());
        }
        observer.OnCheckpointEvent(event);
    }
}

impl CalculatorDeps {
    fn validate(&self) -> Result<(), Error> {
        // Rust 侧 trait 对象已非空；额外校验上游 URI 是否支持增量 meta 扫描。
        validate_incremental_meta_scan_storage(&self.Upstream.URI())
    }
}

impl FileStatistic {
    /// 深拷贝快照，避免后续计数累计污染已发出的事件。
    pub fn snapshot(&self) -> FileStatistic {
        FileStatistic {
            UpstreamReadMetaFileCount: self.UpstreamReadMetaFileCount,
            SkippedStoreSyncedMetaFileCount: self.SkippedStoreSyncedMetaFileCount,
            EstimatedSyncLogFileCount: self.EstimatedSyncLogFileCount,
            DownstreamCheckFileCount: self.DownstreamCheckFileCount,
            PlannedFileSuffixCounts: self.PlannedFileSuffixCounts.clone(),
            DownstreamCheckFileSuffixCounts: self.DownstreamCheckFileSuffixCounts.clone(),
        }
    }

    /// 记录一次下游检查，并按路径后缀聚合计数供指标拆分。
    pub fn record_downstream_check(&mut self, file_path: &str) {
        self.DownstreamCheckFileCount += 1;
        let suffix = crate::progress::path_suffix(file_path);
        *self
            .DownstreamCheckFileSuffixCounts
            .entry(suffix)
            .or_insert(0) += 1;
    }
}
