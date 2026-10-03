// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// GC Worker：TiKV / TiDB 侧的垃圾回收（GC）后台工作者。
//
// GC 清理 MVCC（多版本并发控制）中已过期、不再被任何事务可见的历史版本。
// 本模块负责领导者租约竞选、安全点（safe point）推进、锁解析（resolve locks）、
// delete-range（按键区间删除）以及分布式/中心化/统一 Keyspace GC 等流程。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

/// 系统表中布尔真值的字符串表示。
pub const booleanTrue: &str = "true";
/// 系统表中布尔假值的字符串表示。
pub const booleanFalse: &str = "false";
/// GC Worker 主循环 tick 间隔。
pub const gcWorkerTickInterval: Duration = Duration::from_secs(60);
/// GC 领导者租约时长；持有租约的实例负责调度 GC。
pub const gcWorkerLease: Duration = Duration::from_secs(120);
/// 系统表键：当前 GC 领导者 UUID。
pub const gcLeaderUUIDKey: &str = "tikv_gc_leader_uuid";
/// 系统表键：GC 领导者描述（主机、进程等）。
pub const gcLeaderDescKey: &str = "tikv_gc_leader_desc";
/// 系统表键：GC 领导者租约过期时间。
pub const gcLeaderLeaseKey: &str = "tikv_gc_leader_lease";
/// 系统表键：上次 GC 运行时间。
pub const gcLastRunTimeKey: &str = "tikv_gc_last_run_time";
/// 系统表键：GC 运行间隔配置。
pub const gcRunIntervalKey: &str = "tikv_gc_run_interval";
/// 默认 GC 运行间隔（10 分钟）。
pub const gcDefaultRunInterval: Duration = Duration::from_secs(10 * 60);
/// 两次 GC 作业之间的最短等待时间。
pub const gcWaitTime: Duration = Duration::from_secs(60);
/// redo delete-range 的延迟（默认 24 小时），用于补做已完成区间清理。
pub const gcRedoDeleteRangeDelay: Duration = Duration::from_secs(24 * 60 * 60);
/// 系统表键：GC lifetime（历史版本保留时长）。
pub const gcLifeTimeKey: &str = "tikv_gc_life_time";
/// 默认 GC lifetime。
pub const gcDefaultLifeTime: Duration = Duration::from_secs(10 * 60);
/// GC lifetime 下限，防止保留窗口过短导致可见性问题。
pub const gcMinLifeTime: Duration = Duration::from_secs(10 * 60);
/// 系统表键：GC 安全点时间戳对应的时间。
pub const gcSafePointKey: &str = "tikv_gc_safe_point";
/// 系统表键：GC 并发度配置。
pub const gcConcurrencyKey: &str = "tikv_gc_concurrency";
/// 默认 GC 并发度。
pub const gcDefaultConcurrency: usize = 2;
/// GC 并发度下限。
pub const gcMinConcurrency: usize = 1;
/// GC 并发度上限。
pub const gcMaxConcurrency: usize = 128;
/// 系统表键：是否启用 GC。
pub const gcEnableKey: &str = "tikv_gc_enable";
/// 默认启用 GC。
pub const gcDefaultEnableValue: bool = true;
/// 系统表键：GC 模式（central / distributed）。
pub const gcModeKey: &str = "tikv_gc_mode";
/// 中心化 GC：由 TiDB GC Worker 统一向各 Store 下发清理。
pub const gcModeCentral: &str = "central";
/// 分布式 GC：各 TiKV 自行按安全点清理，TiDB 侧重锁解析与安全点推进。
pub const gcModeDistributed: &str = "distributed";
/// 默认 GC 模式：分布式。
pub const gcModeDefault: &str = gcModeDistributed;
/// 系统表键：扫描锁模式配置。
pub const gcScanLockModeKey: &str = "tikv_gc_scan_lock_mode";
/// 系统表键：是否按 Store 数量自动决定并发度。
pub const gcAutoConcurrencyKey: &str = "tikv_gc_auto_concurrency";
/// 默认开启自动并发度。
pub const gcDefaultAutoConcurrency: bool = true;
/// 向 PD 注册的 GC Worker 服务安全点 ID。
pub const gcWorkerServiceSafePointID: &str = "gc_worker";
/// 状态变量名：上次 GC 运行时间（对外展示）。
pub const tidbGCLastRunTime: &str = "tidb_gc_last_run_time";
/// 状态变量名：GC 领导者描述。
pub const tidbGCLeaderDesc: &str = "tidb_gc_leader_desc";
/// 状态变量名：GC 领导者租约。
pub const tidbGCLeaderLease: &str = "tidb_gc_leader_lease";
/// 状态变量名：GC 领导者 UUID。
pub const tidbGCLeaderUUID: &str = "tidb_gc_leader_uuid";
/// 状态变量名：GC 安全点。
pub const tidbGCSafePoint: &str = "tidb_gc_safe_point";
/// 统一 GC 模式下分批加载 keyspace 的批大小。
pub const loadAllKeyspacesForUnifiedGCBatchSize: u32 = 50;
/// 推进事务安全点后、解析锁前的同步等待时间。
pub const txnSafePointSyncWaitTime: Duration = Duration::from_secs(1);
/// UnsafeDestroyRange RPC 超时。
pub const unsafeDestroyRangeTimeout: Duration = Duration::from_secs(5 * 60);
/// 单次 GC 相关操作超时。
pub const gcTimeout: Duration = Duration::from_secs(5 * 60);
/// 计算 delete-range 并发时对总并发度的除数。
pub const ConcurrencyDivisor: usize = 4;
/// 自动并发下每个线程期望处理的请求/区间规模。
pub const RequestsPerThread: usize = 100_000;
/// 单 Region GC 操作的最大退避时间（毫秒量级配置）。
pub const gcOneRegionMaxBackoff: u64 = 20_000;

#[derive(Clone, Debug, Eq, PartialEq)]
/// GC 模块统一错误类型。
pub struct GCError {
    message: String,
}

impl GCError {
    /// 由错误消息构造 `GCError`。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for GCError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GCError {}

/// GC 操作的统一 Result 别名。
pub type GCResult<T = ()> = Result<T, GCError>;
/// 原始键字节序列。
pub type Key = Vec<u8>;

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
/// 半开键区间 `[start_key, end_key)`。
pub struct KeyRange {
    pub start_key: Key,
    pub end_key: Key,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 待执行的 delete-range 任务（含作业与元素 ID）。
pub struct DelRangeTask {
    pub job_id: i64,
    pub element_id: i64,
    pub range: KeyRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// TiKV Store 生命周期状态。
pub enum StoreState {
    Up,
    Offline,
    Tombstone,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 集群中单个 Store（存储节点）的描述信息。
pub struct StoreInfo {
    pub id: u64,
    pub address: String,
    pub state: StoreState,
    pub engine: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Keyspace（多租户键空间）元信息。
pub struct KeyspaceInfo {
    pub id: u32,
    pub name: String,
    pub enabled: bool,
    pub keyspace_level_gc: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 安全点推进结果：新旧安全点及阻塞原因描述。
pub struct SafePointAdvance {
    pub old_safe_point: u64,
    pub new_safe_point: u64,
    pub blocker_description: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Placement / 标签规则中与区间相关的条目。
pub struct LabelRule {
    pub id: String,
    pub ranges: Vec<KeyRange>,
}

/// GC Worker 访问系统表所需的会话抽象（事务读写配置）。
pub trait GCSession: Send {
    /// 开启事务。
    fn Begin(&mut self) -> GCResult;
    /// 提交事务。
    fn CommitTxn(&mut self) -> GCResult;
    /// 回滚事务。
    fn RollbackTxn(&mut self);
    /// 读取系统表键值。
    fn LoadValue(&mut self, key: &str) -> GCResult<Option<String>>;
    /// 写入系统表键值（带注释）。
    fn SaveValue(&mut self, key: &str, value: &str, comment: &str) -> GCResult;
    /// 关闭会话。
    fn Close(&mut self);
}

/// 包装 `GCSession`，在 Drop 时自动回滚未提交事务并关闭会话。
struct SessionGuard {
    session: Box<dyn GCSession>,
    transaction_active: bool,
}

impl SessionGuard {
    /// 开启事务并标记事务活跃。
    fn Begin(&mut self) -> GCResult {
        self.session.Begin()?;
        self.transaction_active = true;
        Ok(())
    }

    /// 提交事务并清除活跃标记。
    fn CommitTxn(&mut self) -> GCResult {
        let result = self.session.CommitTxn();
        self.transaction_active = false;
        result
    }

    /// 回滚事务并清除活跃标记。
    fn RollbackTxn(&mut self) {
        self.session.RollbackTxn();
        self.transaction_active = false;
    }
}

impl Deref for SessionGuard {
    type Target = dyn GCSession;

    fn deref(&self) -> &Self::Target {
        self.session.as_ref()
    }
}

impl DerefMut for SessionGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.session.as_mut()
    }
}

impl Drop for SessionGuard {
    /// Drop 时若事务仍活跃则回滚，并关闭会话。
    fn drop(&mut self) {
        if self.transaction_active {
            self.session.RollbackTxn();
        }
        self.session.Close();
    }
}

/// GC Worker 对外部运行时的依赖：时间戳、PD 安全点、Store 列表、RPC 等。
pub trait GCWorkerRuntime: Send + Sync + 'static {
    /// 当前集群/存储版本号。
    fn CurrentVersion(&self) -> GCResult<u64>;
    /// 是否对接 TiKV 存储。
    fn IsTiKVStorage(&self) -> bool;
    /// 本机主机名。
    fn HostName(&self) -> Option<String>;
    /// 当前进程 ID。
    fn ProcessID(&self) -> u32;
    /// 当前 Keyspace ID。
    fn KeyspaceID(&self) -> u32;
    /// 是否启用统一 Keyspace GC。
    fn IsUnifiedGC(&self) -> bool;
    /// 是否处于 null keyspace（传统默认空间）。
    fn IsNullKeyspace(&self) -> bool;
    /// Keyspace 名称。
    fn KeyspaceName(&self) -> Option<String>;
    /// 是否为 starter 部署模式。
    fn IsStarterMode(&self) -> bool;
    /// 是否处于测试环境。
    fn InTest(&self) -> bool;
    /// 创建访问系统表的会话。
    fn CreateSession(&self) -> GCResult<Box<dyn GCSession>>;
    /// 注册 GC 相关统计指标。
    fn RegisterStatistics(&self);
    /// 从 Oracle 获取当前物理时间。
    fn OracleTime(&self) -> GCResult<SystemTime>;
    /// 物理时间转为 TSO 时间戳。
    fn TimestampFromTime(&self, time: SystemTime) -> u64;
    /// TSO 时间戳转为物理时间。
    fn TimeFromTimestamp(&self, timestamp: u64) -> SystemTime;
    /// 读取当前 GC 安全点。
    fn GetGCSafePoint(&self) -> GCResult<u64>;
    /// 推进事务安全点（txn safe point）。
    fn AdvanceTxnSafePoint(&self, target: u64) -> GCResult<SafePointAdvance>;
    /// 更新某服务在 PD 上注册的 GC 安全点。
    fn UpdateServiceGCSafePoint(&self, id: &str, safe_point: u64) -> GCResult<u64>;
    /// 推进全局 GC 安全点。
    fn AdvanceGCSafePoint(&self, target: u64) -> GCResult<SafePointAdvance>;
    /// 列出集群全部 Store。
    fn GetAllStores(&self) -> GCResult<Vec<StoreInfo>>;
    /// 是否为 RaftKV v2 引擎。
    fn IsRaftKV2(&self) -> GCResult<bool>;
    /// 加载待执行的 delete-range 任务。
    fn LoadDeleteRanges(&self, safe_point: u64) -> GCResult<Vec<DelRangeTask>>;
    /// 加载指定时间之前已完成、待 redo 的区间。
    fn LoadDoneDeleteRanges(&self, before: u64) -> GCResult<Vec<DelRangeTask>>;
    /// RaftKV v2 上按区间删除。
    fn DeleteRangeRaftV2(
        &self,
        range: &KeyRange,
        concurrency: usize,
        cancel: &CancellationToken,
    ) -> GCResult;
    /// 向 Store 发送 UnsafeDestroyRange，物理销毁区间数据。
    fn UnsafeDestroyRange(
        &self,
        store: &StoreInfo,
        range: &KeyRange,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> GCResult;
    /// 清理与区间相关的 placement 规则。
    fn GCPlacementRules(&self, task: &DelRangeTask) -> GCResult<Vec<i64>>;
    /// 清理与区间相关的标签规则。
    fn GCLabelRules(&self, task: &DelRangeTask) -> GCResult;
    /// 标记 delete-range 任务完成。
    fn CompleteDeleteRange(&self, task: &DelRangeTask, remove_data: bool) -> GCResult;
    /// 删除已完成区间的 redo 记录。
    fn DeleteDoneRecord(&self, task: &DelRangeTask) -> GCResult;
    /// 在指定键区间内解析 safe_point 前的残留锁。
    fn ResolveLocksForRange(
        &self,
        max_version: u64,
        range: &KeyRange,
        concurrency: usize,
        cancel: &CancellationToken,
    ) -> GCResult;
    /// null keyspace 覆盖的键区间列表。
    fn NullKeyspaceRanges(&self) -> Vec<KeyRange>;
    /// 分页加载 keyspace 元信息。
    fn GetAllKeyspaces(&self, start_id: u32, limit: u32) -> GCResult<Vec<KeyspaceInfo>>;
    /// 将 keyspace 编码为其键空间区间。
    fn EncodeKeyspaceRange(&self, keyspace: &KeyspaceInfo) -> GCResult<KeyRange>;
    /// 表 ID 对应的键区间。
    fn TableRange(&self, table_id: i64) -> KeyRange;
    /// 记录某阶段失败。
    fn RecordFailure(&self, stage: &str, error: &GCError);
    /// 记录 GC 事件。
    fn RecordEvent(&self, event: &str);
    /// Controller attached to this runtime's storage, if enabled.
    fn ExternalWorkloadManager(
        &self,
    ) -> Option<Arc<Mutex<Box<dyn astersql_extworkload::Manager>>>> {
        None
    }
}

#[derive(Clone)]
/// 可协作取消的令牌，供后台 GC 任务检查是否应停止。
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    wake: Arc<(Mutex<bool>, Condvar)>,
}

impl CancellationToken {
    /// 创建未取消的令牌。
    fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            wake: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    /// 是否已被取消。
    pub fn IsCancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// 标记取消并唤醒等待者。
    pub fn Cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.wake.1.notify_all();
    }

    /// 等待至多 `duration`；若期间被取消返回 `false`，否则 `true`。
    pub fn Wait(&self, duration: Duration) -> bool {
        if self.IsCancelled() {
            return false;
        }
        let guard = self.wake.0.lock().expect("gc cancellation lock poisoned");
        let _ = self
            .wake
            .1
            .wait_timeout_while(guard, duration, |_| !self.IsCancelled())
            .expect("gc cancellation lock poisoned");
        !self.IsCancelled()
    }

    /// 若已取消则返回错误。
    fn Check(&self) -> GCResult {
        if self.IsCancelled() {
            Err(GCError::new("gc worker cancelled"))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// GC 并发配置：并发度数值与是否自动模式。
pub struct gcConcurrency {
    pub v: usize,
    pub isAuto: bool,
}

#[derive(Debug)]
/// Worker 运行时状态：是否正在执行 GC、上次完成时间等。
struct WorkerState {
    gc_is_running: bool,
    last_finish: Instant,
    has_finished_first_gc_job: bool,
}

#[derive(Clone)]
/// GC Worker 主体：持有领导者身份、运行时依赖与后台线程句柄。
pub struct GCWorker {
    pub uuid: String,
    pub desc: String,
    pub keyspaceID: u32,
    runtime: Arc<dyn GCWorkerRuntime>,
    state: Arc<Mutex<WorkerState>>,
    cancel: CancellationToken,
    handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

/// 创建并注册统计的 GC Worker；仅允许针对 TiKV 存储运行。
pub fn NewGCWorker(runtime: Arc<dyn GCWorkerRuntime>) -> GCResult<GCWorker> {
    let version = runtime.CurrentVersion()?;
    if !runtime.IsTiKVStorage() {
        return Err(GCError::new("GC should run against TiKV storage"));
    }
    let host = runtime.HostName().unwrap_or_else(|| "unknown".to_owned());
    let worker = GCWorker {
        uuid: format!("{version:x}"),
        desc: format!(
            "host:{host}, pid:{}, start at {:?}",
            runtime.ProcessID(),
            SystemTime::now()
        ),
        keyspaceID: runtime.KeyspaceID(),
        runtime,
        state: Arc::new(Mutex::new(WorkerState {
            gc_is_running: false,
            last_finish: Instant::now(),
            has_finished_first_gc_job: false,
        })),
        cancel: CancellationToken::new(),
        handle: Arc::new(Mutex::new(None)),
    };
    worker.runtime.RegisterStatistics();
    Ok(worker)
}

impl GCWorker {
    /// 启动后台 tick 循环（幂等，已启动则直接返回）。
    pub fn Start(&self) {
        let mut handle = self.handle.lock().expect("gc worker handle lock poisoned");
        if handle.is_some() {
            return;
        }
        // 克隆 worker 供后台线程使用。
        let worker = self.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        *handle = Some(thread::spawn(move || worker.start(started_tx)));
        drop(handle);
        let _ = started_rx.recv();
    }

    /// 取消并等待后台线程结束。
    pub fn Close(&self) {
        self.cancel.Cancel();
        if let Some(handle) = self
            .handle
            .lock()
            .expect("gc worker handle lock poisoned")
            .take()
        {
            let _ = handle.join();
        }
    }

    /// 后台主循环：周期性 tick，并收集 GC 作业完成通知。
    fn start(&self, started: mpsc::SyncSender<()>) {
        let (done_tx, done_rx) = mpsc::channel::<GCResult>();
        self.tick(&done_tx);
        let _ = started.send(());
        let mut next_tick = Instant::now() + gcWorkerTickInterval;
        // 直到取消：处理完成通知，并按 tick 间隔调度 leaderTick。
        while !self.cancel.IsCancelled() {
            match done_rx.try_recv() {
                // GC 作业线程完成：更新状态并记录失败。
                Ok(result) => {
                    let mut state = self.state.lock().expect("gc worker state lock poisoned");
                    state.gc_is_running = false;
                    state.last_finish = Instant::now();
                    state.has_finished_first_gc_job = true;
                    drop(state);
                    if let Err(error) = result {
                        self.runtime.RecordFailure("run_gc_job", &error);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
            let now = Instant::now();
            // 到达下一个 tick 点则再次调度。
            if now >= next_tick {
                self.tick(&done_tx);
                next_tick = now + gcWorkerTickInterval;
            }
            let until_tick = next_tick.saturating_duration_since(Instant::now());
            if !self.cancel.Wait(until_tick.min(Duration::from_millis(100))) {
                break;
            }
        }
    }

    /// 创建内部会话；失败则短暂等待后重试，直至取消。
    fn createSession(&self) -> SessionGuard {
        loop {
            match self.runtime.CreateSession() {
                Ok(session) => {
                    return SessionGuard {
                        session,
                        transaction_active: false,
                    };
                }
                Err(error) => {
                    self.runtime.RecordFailure("create_session", &error);
                    if !self.cancel.Wait(Duration::from_millis(10)) {
                        panic!("gc worker cancelled while creating its internal session");
                    }
                }
            }
        }
    }

    /// 状态变量作用域占位（与 Go 侧兼容）。
    pub fn GetScope(&self, _status: &str) -> u8 {
        0
    }

    /// 从系统表加载 GC 相关状态，映射为 tidb_gc_* 状态变量。
    pub fn Stats(&self) -> HashMap<String, String> {
        let mut stats = HashMap::new();
        for (source, target) in [
            (gcLeaderUUIDKey, tidbGCLeaderUUID),
            (gcLeaderDescKey, tidbGCLeaderDesc),
            (gcLeaderLeaseKey, tidbGCLeaderLease),
            (gcLastRunTimeKey, tidbGCLastRunTime),
            (gcSafePointKey, tidbGCSafePoint),
        ] {
            if let Ok(value) = self.loadValueFromSysTable(source) {
                stats.insert(target.to_owned(), value);
            }
        }
        stats
    }

    /// 单次 tick：检查领导者身份并在成为领导者时执行 `leaderTick`。
    fn tick(&self, done: &mpsc::Sender<GCResult>) {
        match self.checkLeader() {
            // 当前实例是 GC 领导者。
            Ok(true) => {
                if let Err(error) = self.leaderTick(done) {
                    self.runtime.RecordFailure("leader_tick", &error);
                }
            }
            // 非领导者，跳过本轮调度。
            Ok(false) => self.runtime.RecordEvent("not_leader"),
            Err(error) => self.runtime.RecordFailure("check_leader", &error),
        }
    }

    /// 判断距上次 GC 完成是否仍需等待 `gcWaitTime`。
    pub(crate) fn needsToWait(&self) -> bool {
        let state = self.state.lock().expect("gc worker state lock poisoned");
        if self.runtime.IsStarterMode() && !self.runtime.InTest() {
            state.last_finish.elapsed() < gcWaitTime && state.has_finished_first_gc_job
        } else {
            state.last_finish.elapsed() < gcWaitTime
        }
    }

    /// 领导者侧调度：准备安全点并异步启动 GC 作业。
    fn leaderTick(&self, done: &mpsc::Sender<GCResult>) -> GCResult {
        if self
            .state
            .lock()
            .expect("gc worker state lock poisoned")
            .gc_is_running
        {
            return Ok(());
        }
        let concurrency = self.getGCConcurrency()?;
        // 统一 Keyspace GC：走 keyspace 级 delete-range 路径。
        if self.runtime.IsUnifiedGC() {
            return self.runKeyspaceGCJobInUnifiedGCMode(concurrency, done);
        }
        // prepare 返回 None 表示本轮无需启动 GC。
        let Some(safe_point) = self.prepare()? else {
            return Ok(());
        };
        // 距上次完成过近，避免过于频繁地启动作业。
        if self.needsToWait() {
            return Ok(());
        }
        self.state
            .lock()
            .expect("gc worker state lock poisoned")
            .gc_is_running = true;
        let worker = self.clone();
        let done = done.clone();
        // 异步执行 GC 作业，完成后通过 channel 回报。
        thread::spawn(move || {
            let _ = done.send(worker.runGCJob(safe_point, concurrency));
        });
        Ok(())
    }

    /// 统一 Keyspace GC 模式：按间隔触发 keyspace 级 delete-range。
    fn runKeyspaceGCJobInUnifiedGCMode(
        &self,
        concurrency: gcConcurrency,
        done: &mpsc::Sender<GCResult>,
    ) -> GCResult {
        if self.needsToWait() {
            return Ok(());
        }
        let now = self.getOracleTime()?;
        if !self.checkGCInterval(now)? {
            return Ok(());
        }
        let worker = self.clone();
        let done = done.clone();
        thread::spawn(move || {
            let _ = done.send(worker.runKeyspaceDeleteRange(concurrency));
        });
        self.saveTime(gcLastRunTimeKey, now)
    }

    /// 在事务中执行 prepare；成功则提交并返回新安全点。
    pub(crate) fn prepare(&self) -> GCResult<Option<u64>> {
        let mut session = self.createSession();
        session.Begin()?;
        let result = self.checkPrepare();
        match &result {
            // 成功算出安全点才提交事务，否则回滚。
            Ok(Some(_)) => session.CommitTxn()?,
            _ => session.RollbackTxn(),
        }
        result
    }

    /// 检查启用状态、运行间隔并计算新的事务安全点。
    fn checkPrepare(&self) -> GCResult<Option<u64>> {
        if !self.checkGCEnable()? {
            return Ok(None);
        }
        let now = self.getOracleTime()?;
        if !self.checkGCInterval(now)? {
            return Ok(None);
        }
        let Some((safe_point_time, safe_point)) = self.calcNewTxnSafePoint(now)? else {
            return Ok(None);
        };
        self.saveTime(gcLastRunTimeKey, now)?;
        self.saveTime(gcSafePointKey, safe_point_time)?;
        Ok(Some(safe_point))
    }

    /// 获取 Oracle（全局时间戳服务）当前物理时间。
    pub(crate) fn getOracleTime(&self) -> GCResult<SystemTime> {
        self.runtime.OracleTime()
    }

    /// 读取是否启用 GC。
    fn checkGCEnable(&self) -> GCResult<bool> {
        self.loadBooleanWithDefault(gcEnableKey, gcDefaultEnableValue)
    }

    /// 读取是否使用自动并发度。
    fn checkUseAutoConcurrency(&self) -> GCResult<bool> {
        self.loadBooleanWithDefault(gcAutoConcurrencyKey, gcDefaultAutoConcurrency)
    }

    /// 从系统表加载布尔配置；缺失时写入默认值。
    fn loadBooleanWithDefault(&self, key: &str, default_value: bool) -> GCResult<bool> {
        let value = self.loadValueFromSysTable(key)?;
        if value.is_empty() {
            self.saveValueToSysTable(
                key,
                if default_value {
                    booleanTrue
                } else {
                    booleanFalse
                },
            )?;
            return Ok(default_value);
        }
        Ok(value.eq_ignore_ascii_case(booleanTrue))
    }

    /// 解析 GC 并发配置：固定值或按可用 Store 数自动估算。
    pub(crate) fn getGCConcurrency(&self) -> GCResult<gcConcurrency> {
        let use_auto = self
            .checkUseAutoConcurrency()
            .unwrap_or(gcDefaultAutoConcurrency);
        // 固定并发：直接读取配置值。
        if !use_auto {
            return Ok(gcConcurrency {
                v: self.loadGCConcurrencyWithDefault()?,
                isAuto: false,
            });
        }
        // 自动并发：优先按可用 Store 数量估算。
        let concurrency = match self.getStoresForGC() {
            Ok(stores) => stores.len(),
            Err(_) => self
                .loadGCConcurrencyWithDefault()
                .unwrap_or(gcDefaultConcurrency),
        };
        if concurrency == 0 {
            return Err(GCError::new("[gc worker] no store is up"));
        }
        Ok(gcConcurrency {
            v: concurrency,
            isAuto: true,
        })
    }

    /// 判断是否已到达下一次允许运行的时间间隔。
    fn checkGCInterval(&self, now: SystemTime) -> GCResult<bool> {
        let interval = self.loadDurationWithDefault(gcRunIntervalKey, gcDefaultRunInterval)?;
        let Some(last_run) = self.loadTime(gcLastRunTimeKey)? else {
            return Ok(true);
        };
        Ok(last_run
            .checked_add(interval)
            .is_none_or(|next_run| next_run <= now))
    }

    /// 校验并必要时抬升 GC lifetime 至下限。
    fn validateGCLifeTime(&self, lifetime: Duration) -> GCResult<Duration> {
        if lifetime >= gcMinLifeTime {
            return Ok(lifetime);
        }
        self.saveDuration(gcLifeTimeKey, gcMinLifeTime)?;
        Ok(gcMinLifeTime)
    }

    /// 按 lifetime 计算目标时间戳并尝试推进事务安全点。
    fn calcNewTxnSafePoint(&self, now: SystemTime) -> GCResult<Option<(SystemTime, u64)>> {
        let lifetime = self
            .validateGCLifeTime(self.loadDurationWithDefault(gcLifeTimeKey, gcDefaultLifeTime)?)?;
        let target_time = now
            .checked_sub(lifetime)
            .ok_or_else(|| GCError::new("gc lifetime precedes the system clock epoch"))?;
        let target = self.runtime.TimestampFromTime(target_time);
        let new_safe_point = match self.advanceTxnSafePoint(target) {
            // 安全点不能回退；忽略本轮推进。
            Err(error) if error.to_string().contains("ErrDecreasingTxnSafePoint") => {
                return Ok(None);
            }
            result => result?,
        };
        if new_safe_point == 0 {
            return Ok(None);
        }
        Ok(Some((
            self.runtime.TimeFromTimestamp(new_safe_point),
            new_safe_point,
        )))
    }

    /// 向 PD 推进事务安全点；未前进时返回 0。
    fn advanceTxnSafePoint(&self, target: u64) -> GCResult<u64> {
        let result = self.runtime.AdvanceTxnSafePoint(target)?;
        if result.new_safe_point <= result.old_safe_point {
            return Ok(0);
        }
        Ok(result.new_safe_point)
    }

    /// 更新本 Worker 的服务安全点，返回与目标的较小值。
    pub(crate) fn setGCWorkerServiceSafePoint(&self, safe_point: u64) -> GCResult<u64> {
        self.runtime
            .UpdateServiceGCSafePoint(gcWorkerServiceSafePointID, safe_point)
            .map(|minimum| minimum.min(safe_point))
    }

    /// 执行一轮完整 GC：解析锁 → delete-range → redo → 广播安全点。
    pub(crate) fn runGCJob(&self, safe_point: u64, concurrency: gcConcurrency) -> GCResult {
        self.runtime.RecordEvent("run_job");
        if !self.cancel.Wait(txnSafePointSyncWaitTime) {
            return Err(GCError::new("gc worker cancelled before resolving locks"));
        }
        // 阶段 1：解析 safe_point 之前残留的未决锁。
        self.resolveLocks(safe_point, concurrency.v)
            .map_err(|error| self.stageError("resolve_lock", error))?;
        // 阶段 2：按区间物理删除过期数据。
        self.deleteRanges(safe_point, concurrency)
            .map_err(|error| self.stageError("delete_range", error))?;
        // 阶段 3：补做历史已完成的 delete-range。
        self.redoDeleteRanges(safe_point, concurrency)
            .map_err(|error| self.stageError("redo_delete_range", error))?;
        // 阶段 4：向 PD/集群广播新的 GC 安全点。
        let gc_safe_point = self
            .broadcastGCSafePoint(safe_point)
            .map_err(|error| self.stageError("upload_safe_point", error))?;
        self.notifyGCV2AfterGC(gc_safe_point);
        Ok(())
    }

    /// Notify the controller only after PD successfully advances the GC safe point.
    pub(crate) fn notifyGCV2AfterGC(&self, safe_point: u64) {
        let Some(manager) = self.runtime.ExternalWorkloadManager() else {
            return;
        };
        let mut manager = manager
            .lock()
            .expect("external workload manager lock poisoned");
        if !astersql_extworkload::IsKeyspaceUsingKeyspaceLevelGC(manager.Meta()) {
            return;
        }
        let role = manager.Role();
        let ctx = astersql_extworkload::context::Background();
        if matches!(role.as_str(), "master" | "ttl" | "gcv2") {
            if let Err(error) = manager.RecycleGCV2(&ctx, safe_point) {
                self.runtime
                    .RecordEvent(&format!("failed_recycle_gcv2: {error}"));
            }
        }
        if matches!(role.as_str(), "master" | "ttl") {
            match self.loadDurationWithDefault(gcLifeTimeKey, gcDefaultLifeTime) {
                Err(error) => self
                    .runtime
                    .RecordEvent(&format!("failed_load_gcv2_lifetime: {error}")),
                Ok(lifetime) => {
                    if let Err(error) = manager.RegisterGCV2(&ctx, safe_point, lifetime) {
                        self.runtime
                            .RecordEvent(&format!("failed_register_gcv2: {error}"));
                    }
                }
            }
        }
    }

    /// 统一 GC 模式下仅执行 delete-range / redo（安全点由别处推进）。
    fn runKeyspaceDeleteRange(&self, concurrency: gcConcurrency) -> GCResult {
        let safe_point = match self.runtime.GetGCSafePoint() {
            Ok(0) | Err(_) => return Ok(()),
            Ok(safe_point) => safe_point,
        };
        self.logIsGCSafePointTooEarly(safe_point).ok();
        self.deleteRanges(safe_point, concurrency)?;
        self.redoDeleteRanges(safe_point, concurrency)
    }

    /// 若安全点相对当前时间过于陈旧则记录事件。
    fn logIsGCSafePointTooEarly(&self, safe_point: u64) -> GCResult {
        let now = self.getOracleTime()?;
        let check_time = now
            .checked_sub(gcDefaultLifeTime * 2)
            .ok_or_else(|| GCError::new("gc time underflow"))?;
        if self.runtime.TimestampFromTime(check_time) > safe_point {
            self.runtime.RecordEvent("gc_safe_point_too_early");
        }
        Ok(())
    }

    /// 加载并并发执行待删除区间，清理 placement/label 规则后标记完成。
    pub(crate) fn deleteRanges(&self, safe_point: u64, concurrency: gcConcurrency) -> GCResult {
        self.runtime.RecordEvent("delete_range");
        let ranges = self.runtime.LoadDeleteRanges(safe_point)?;
        let raft_v2 = self.runtime.IsRaftKV2()?;
        let limit = self.calcDeleteRangeConcurrency(concurrency, ranges.len());
        let placement_cache = Arc::new(Mutex::new(HashSet::<i64>::new()));
        self.forEachRange(ranges, limit, |task| {
            // RaftKV v2 使用专用 DeleteRange；否则走 UnsafeDestroyRange。
            let range_result = if raft_v2 {
                self.runtime
                    .DeleteRangeRaftV2(&task.range, limit, &self.cancel)
            } else {
                self.doUnsafeDestroyRangeRequest(&task.range)
            };
            if let Err(error) = range_result {
                self.runtime.RecordFailure("delete_range_item", &error);
                return;
            }
            match self.runtime.GCPlacementRules(task) {
                Ok(ids) => {
                    placement_cache
                        .lock()
                        .expect("placement cache lock poisoned")
                        .extend(ids);
                }
                Err(error) => {
                    self.runtime.RecordFailure("gc_placement_rules", &error);
                    return;
                }
            }
            if let Err(error) = self.runtime.GCLabelRules(task) {
                self.runtime.RecordFailure("gc_label_rules", &error);
                return;
            }
            if let Err(error) = self.runtime.CompleteDeleteRange(task, !raft_v2) {
                self.runtime.RecordFailure("complete_delete_range", &error);
            }
        });
        Ok(())
    }

    /// 根据总并发与区间数量计算 delete-range 实际并发度。
    pub fn calcDeleteRangeConcurrency(
        &self,
        concurrency: gcConcurrency,
        range_num: usize,
    ) -> usize {
        let maximum = (concurrency.v / ConcurrencyDivisor).max(1);
        let request_based = (range_num / RequestsPerThread).max(1);
        if concurrency.isAuto {
            maximum.min(request_based)
        } else {
            maximum
        }
    }

    /// 对延迟窗口之前已完成的 delete-range 记录做补做清理。
    fn redoDeleteRanges(&self, safe_point: u64, concurrency: gcConcurrency) -> GCResult {
        let delay = self
            .runtime
            .TimestampFromTime(SystemTime::UNIX_EPOCH + gcRedoDeleteRangeDelay);
        let before = safe_point.saturating_sub(delay);
        let ranges = self.runtime.LoadDoneDeleteRanges(before)?;
        let limit = self.calcDeleteRangeConcurrency(concurrency, ranges.len());
        self.forEachRange(ranges, limit, |task| {
            if let Err(error) = self.doUnsafeDestroyRangeRequest(&task.range) {
                self.runtime.RecordFailure("redo_delete_range_item", &error);
                return;
            }
            if let Err(error) = self.runtime.DeleteDoneRecord(task) {
                self.runtime.RecordFailure("delete_done_record", &error);
            }
        });
        Ok(())
    }

    /// 以有限并发对区间列表执行回调（工作窃取式取下标）。
    fn forEachRange<F>(&self, ranges: Vec<DelRangeTask>, concurrency: usize, action: F)
    where
        F: Fn(&DelRangeTask) + Sync,
    {
        let next = AtomicUsize::new(0);
        let ranges = &ranges;
        // 向每个 Store 并发发送销毁请求。
        thread::scope(|scope| {
            // 启动有限个 worker，通过原子下标领取任务。
            for _ in 0..concurrency.max(1).min(ranges.len().max(1)) {
                let action = &action;
                let next = &next;
                scope.spawn(move || {
                    loop {
                        let index = next.fetch_add(1, Ordering::AcqRel);
                        let Some(task) = ranges.get(index) else {
                            break;
                        };
                        action(task);
                    }
                });
            }
        });
    }

    /// 向所有需 GC 的 Store 并发发送 UnsafeDestroyRange。
    fn doUnsafeDestroyRangeRequest(&self, range: &KeyRange) -> GCResult {
        let stores = self.getStoresForGC()?;
        let errors = Arc::new(Mutex::new(Vec::new()));
        thread::scope(|scope| {
            for store in &stores {
                let errors = Arc::clone(&errors);
                scope.spawn(move || {
                    if let Err(error) = self.runtime.UnsafeDestroyRange(
                        store,
                        range,
                        unsafeDestroyRangeTimeout,
                        &self.cancel,
                    ) {
                        errors
                            .lock()
                            .expect("destroy range error lock poisoned")
                            .push(error.to_string());
                    }
                });
            }
        });
        let errors = errors.lock().expect("destroy range error lock poisoned");
        if errors.is_empty() {
            Ok(())
        } else {
            Err(GCError::new(format!(
                "[gc worker] destroy range finished with errors: {errors:?}"
            )))
        }
    }

    /// 过滤出需要参与 GC 操作的 Store（排除 tombstone 等）。
    fn getStoresForGC(&self) -> GCResult<Vec<StoreInfo>> {
        self.runtime
            .GetAllStores()?
            .into_iter()
            .filter_map(|store| match needsGCOperationForStore(&store) {
                Ok(true) => Some(Ok(store)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    /// 加载并发配置并钳制在合法区间；缺失则写入默认值。
    fn loadGCConcurrencyWithDefault(&self) -> GCResult<usize> {
        let value = self.loadValueFromSysTable(gcConcurrencyKey)?;
        if value.is_empty() {
            self.saveValueToSysTable(gcConcurrencyKey, &gcDefaultConcurrency.to_string())?;
            return Ok(gcDefaultConcurrency);
        }
        let parsed = value
            .parse::<usize>()
            .map_err(|error| GCError::new(error.to_string()))?;
        Ok(parsed.clamp(gcMinConcurrency, gcMaxConcurrency))
    }

    /// 检查/初始化 GC 模式；当前实现始终视为分布式可用。
    pub fn checkUseDistributedGC(&self) -> bool {
        let mode = self.loadValueFromSysTable(gcModeKey);
        if matches!(&mode, Ok(value) if value.is_empty()) {
            self.saveValueToSysTable(gcModeKey, gcModeDefault).ok();
        }
        true
    }

    /// 按 keyspace 策略在安全点前解析残留事务锁。
    pub(crate) fn resolveLocks(&self, txn_safe_point: u64, concurrency: usize) -> GCResult {
        self.cancel.Check()?;
        let max_version = txn_safe_point.wrapping_sub(1);
        // 非 null keyspace：对当前空间整段解析锁即可。
        if !self.runtime.IsNullKeyspace() {
            return self.runtime.ResolveLocksForRange(
                max_version,
                &KeyRange::default(),
                concurrency,
                &self.cancel,
            );
        }
        let mut batch = self
            .runtime
            .GetAllKeyspaces(0, loadAllKeyspacesForUnifiedGCBatchSize)?;
        if batch.is_empty() {
            return self.runtime.ResolveLocksForRange(
                max_version,
                &KeyRange::default(),
                concurrency,
                &self.cancel,
            );
        }
        // null keyspace / 统一 GC：分别处理 null 区间与各 keyspace。
        let mut completely_successful = true;
        for range in self.runtime.NullKeyspaceRanges() {
            if self
                .runtime
                .ResolveLocksForRange(max_version, &range, concurrency, &self.cancel)
                .is_err()
            {
                completely_successful = false;
            }
        }
        loop {
            for keyspace in &batch {
                // 跳过禁用或已由 keyspace 自行 GC 的空间。
                if !keyspace.enabled || keyspace.keyspace_level_gc {
                    continue;
                }
                let result = self
                    .runtime
                    .EncodeKeyspaceRange(keyspace)
                    .and_then(|range| {
                        self.runtime.ResolveLocksForRange(
                            max_version,
                            &range,
                            concurrency,
                            &self.cancel,
                        )
                    });
                if result.is_err() {
                    completely_successful = false;
                }
            }
            let Some(next_id) = batch.last().and_then(|keyspace| keyspace.id.checked_add(1)) else {
                break;
            };
            batch = self
                .runtime
                .GetAllKeyspaces(next_id, loadAllKeyspacesForUnifiedGCBatchSize)?;
            if batch.is_empty() {
                break;
            }
        }
        if completely_successful {
            Ok(())
        } else {
            Err(GCError::new("resolve locks is not completely successful"))
        }
    }

    /// 将新的 GC 安全点推进到 PD。
    fn broadcastGCSafePoint(&self, safe_point: u64) -> GCResult<u64> {
        self.runtime
            .AdvanceGCSafePoint(safe_point)
            .map(|result| result.new_safe_point)
    }

    /// 竞选或续租 GC 领导者；成功返回 true。
    pub(crate) fn checkLeader(&self) -> GCResult<bool> {
        let mut session = self.createSession();
        session.Begin()?;
        let leader = session.LoadValue(gcLeaderUUIDKey)?.unwrap_or_default();
        // 已是领导者：续租并提交。
        if leader == self.uuid {
            let lease = SystemTime::now() + gcWorkerLease;
            session.SaveValue(
                gcLeaderLeaseKey,
                &format_system_time(lease),
                gcVariableComment(gcLeaderLeaseKey),
            )?;
            let result = session.CommitTxn();
            return result.map(|()| true);
        }
        session.RollbackTxn();
        session.Begin()?;
        let lease = session.LoadValue(gcLeaderLeaseKey)?;
        let expired = lease
            .as_deref()
            .map(parse_system_time)
            .transpose()?
            .is_none_or(|lease| lease < SystemTime::now());
        // 租约未过期，其他实例仍是领导者。
        if !expired {
            session.RollbackTxn();
            return Ok(false);
        }
        session.SaveValue(
            gcLeaderUUIDKey,
            &self.uuid,
            gcVariableComment(gcLeaderUUIDKey),
        )?;
        session.SaveValue(
            gcLeaderDescKey,
            &self.desc,
            gcVariableComment(gcLeaderDescKey),
        )?;
        session.SaveValue(
            gcLeaderLeaseKey,
            &format_system_time(SystemTime::now() + gcWorkerLease),
            gcVariableComment(gcLeaderLeaseKey),
        )?;
        let result = session.CommitTxn();
        result.map(|()| true)
    }

    /// 将时间写入系统表。
    fn saveTime(&self, key: &str, time: SystemTime) -> GCResult {
        self.saveValueToSysTable(key, &format_system_time(time))
    }

    /// 从系统表读取时间。
    fn loadTime(&self, key: &str) -> GCResult<Option<SystemTime>> {
        let value = self.loadValueFromSysTable(key)?;
        if value.is_empty() {
            Ok(None)
        } else {
            parse_system_time(&value).map(Some)
        }
    }

    /// 将时长写入系统表。
    fn saveDuration(&self, key: &str, duration: Duration) -> GCResult {
        self.saveValueToSysTable(key, &format_duration(duration))
    }

    /// 从系统表读取时长。
    fn loadDuration(&self, key: &str) -> GCResult<Option<Duration>> {
        let value = self.loadValueFromSysTable(key)?;
        if value.is_empty() {
            Ok(None)
        } else {
            parse_duration(&value).map(Some)
        }
    }

    /// 读取时长配置；缺失则写入默认值。
    fn loadDurationWithDefault(&self, key: &str, default: Duration) -> GCResult<Duration> {
        match self.loadDuration(key)? {
            Some(duration) => Ok(duration),
            None => {
                self.saveDuration(key, default)?;
                Ok(default)
            }
        }
    }

    /// 从系统表加载字符串配置。
    fn loadValueFromSysTable(&self, key: &str) -> GCResult<String> {
        let mut session = self.createSession();
        let result = session
            .LoadValue(key)
            .map(|value| value.unwrap_or_default());
        result
    }

    /// 将字符串配置写入系统表。
    fn saveValueToSysTable(&self, key: &str, value: &str) -> GCResult {
        let mut session = self.createSession();
        let result = session.SaveValue(key, value, gcVariableComment(key));
        result
    }

    /// 包装阶段名后记录并返回错误。
    fn stageError(&self, stage: &str, error: GCError) -> GCError {
        self.runtime.RecordFailure(stage, &error);
        error
    }
}

impl Drop for GCWorker {
    /// Worker Drop 时关闭后台线程。
    fn drop(&mut self) {
        if Arc::strong_count(&self.handle) == 1 {
            self.Close();
        }
    }
}

/// 判断该 Store 是否需要参与 GC（TiKV/TiFlash 且非 Tombstone）。
pub fn needsGCOperationForStore(store: &StoreInfo) -> GCResult<bool> {
    if store.state == StoreState::Tombstone {
        return Ok(false);
    }
    match store.engine.as_str() {
        "tiflash" | "tiflash_compute" => Ok(false),
        "" | "tikv" => Ok(true),
        engine => Err(GCError::new(format!(
            "unsupported store engine {engine:?} with storeID {}, addr {}",
            store.id, store.address
        ))),
    }
}

/// 便捷函数：读取 GC 安全点。
pub fn getGCSafePoint(runtime: &dyn GCWorkerRuntime) -> GCResult<u64> {
    runtime.GetGCSafePoint()
}

/// 加载与表相关的 GC placement 规则 ID。
pub fn getGCRules(
    ids: &[i64],
    rules: &HashMap<String, LabelRule>,
    runtime: &dyn GCWorkerRuntime,
) -> Vec<String> {
    let ranges: HashSet<KeyRange> = ids.iter().map(|id| runtime.TableRange(*id)).collect();
    rules
        .values()
        .filter(|rule| rule.ranges.iter().any(|range| ranges.contains(range)))
        .map(|rule| rule.id.clone())
        .collect()
}

/// 以给定安全点与并发度运行一轮中心化风格 GC 作业。
pub fn RunGCJob(
    runtime: Arc<dyn GCWorkerRuntime>,
    safe_point: u64,
    identifier: &str,
    concurrency: usize,
) -> GCResult {
    RunDistributedGCJob(runtime, safe_point, identifier, concurrency)
}

/// 运行分布式 GC：解析锁并广播安全点。
pub fn RunDistributedGCJob(
    runtime: Arc<dyn GCWorkerRuntime>,
    safe_point: u64,
    identifier: &str,
    concurrency: usize,
) -> GCResult {
    let mut worker = NewGCWorker(runtime)?;
    worker.uuid = identifier.to_owned();
    let new_safe_point = worker.advanceTxnSafePoint(safe_point)?;
    if !worker.cancel.Wait(txnSafePointSyncWaitTime) {
        return Err(GCError::new("distributed GC cancelled"));
    }
    worker.resolveLocks(new_safe_point, concurrency)?;
    worker.broadcastGCSafePoint(new_safe_point).map(|_| ())
}

/// 仅执行指定区间上的锁解析。
pub fn RunResolveLocks(
    runtime: Arc<dyn GCWorkerRuntime>,
    safe_point: u64,
    identifier: &str,
    concurrency: usize,
) -> GCResult {
    let mut worker = NewGCWorker(runtime)?;
    worker.uuid = identifier.to_owned();
    worker.resolveLocks(safe_point, concurrency)
}

/// 测试用 Mock GC Worker，复用真实 Worker 的部分能力。
pub struct MockGCWorker {
    worker: GCWorker,
}

/// 创建 MockGCWorker。
pub fn NewMockGCWorker(runtime: Arc<dyn GCWorkerRuntime>) -> GCResult<MockGCWorker> {
    Ok(MockGCWorker {
        worker: NewGCWorker(runtime)?,
    })
}

impl MockGCWorker {
    /// Mock：对安全点前的区间执行 delete-range。
    pub fn DeleteRanges(&self, safe_point: u64) -> GCResult {
        self.worker.deleteRanges(
            safe_point,
            gcConcurrency {
                v: 1,
                isAuto: false,
            },
        )
    }
}

/// 系统表各 GC 配置键的人类可读注释。
fn gcVariableComment(key: &str) -> &'static str {
    match key {
        gcLeaderUUIDKey => "Current GC worker leader UUID. (DO NOT EDIT)",
        gcLeaderDescKey => "Host name and pid of current GC leader. (DO NOT EDIT)",
        gcLeaderLeaseKey => "Current GC worker leader lease. (DO NOT EDIT)",
        gcLastRunTimeKey => "The time when last GC starts. (DO NOT EDIT)",
        gcRunIntervalKey => "GC run interval, at least 10m, in Go format.",
        gcLifeTimeKey => {
            "All versions within life time will not be collected by GC, at least 10m, in Go format."
        }
        gcSafePointKey => "All versions after safe point can be accessed. (DO NOT EDIT)",
        gcConcurrencyKey => "How many goroutines used to do GC parallel, [1, 128], default 2",
        gcEnableKey => "Current GC enable status",
        gcModeKey => "Mode of GC, central or distributed. (Obsolete)",
        gcAutoConcurrencyKey => "Let TiDB pick the concurrency automatically",
        gcScanLockModeKey => "Mode of scanning locks. (Deprecated)",
        _ => "",
    }
}

/// 按 Go `tikvutil.GCTimeFormat` 将时间以 UTC 写入系统表。
pub(crate) fn format_system_time(time: SystemTime) -> String {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = duration.as_secs() as i64;
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let seconds_of_day = seconds.rem_euclid(86_400);
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;
    format!(
        "{year:04}{month:02}{day:02}-{hour:02}:{minute:02}:{second:02}.{:03} +0000",
        duration.subsec_millis()
    )
}

/// 解析 Go `CompatibleParseGCTime` 所接受的当前/旧版 GC 时间文本。
pub(crate) fn parse_system_time(value: &str) -> GCResult<SystemTime> {
    // Earlier Rust ports wrote Unix milliseconds. Keep that read path so an
    // upgrade can consume its existing rows, while all new writes use the Go
    // wire format above.
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        let milliseconds = value
            .parse::<u64>()
            .map_err(|error| GCError::new(error.to_string()))?;
        return Ok(SystemTime::UNIX_EPOCH + Duration::from_millis(milliseconds));
    }
    let mut fields = value.split_whitespace();
    let date_time = fields
        .next()
        .ok_or_else(|| GCError::new("invalid GC time"))?;
    let offset = fields
        .next()
        .ok_or_else(|| GCError::new("GC time has no timezone offset"))?;
    let (date, clock) = date_time
        .split_once('-')
        .ok_or_else(|| GCError::new("invalid GC time"))?;
    if date.len() != 8 || clock.len() < 8 {
        return Err(GCError::new("invalid GC time"));
    }
    let parse = |text: &str| {
        text.parse::<i64>()
            .map_err(|error| GCError::new(error.to_string()))
    };
    let year = parse(&date[0..4])?;
    let month = parse(&date[4..6])?;
    let day = parse(&date[6..8])?;
    let hour = parse(&clock[0..2])?;
    let minute = parse(&clock[3..5])?;
    let second = parse(&clock[6..8])?;
    if &clock[2..3] != ":"
        || &clock[5..6] != ":"
        || !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return Err(GCError::new("invalid GC time"));
    }
    let fraction_nanos = match clock.get(8..) {
        Some("") | None => 0,
        Some(fraction) if fraction.starts_with('.') => {
            let digits = &fraction[1..];
            if digits.is_empty()
                || digits.len() > 9
                || !digits.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(GCError::new("invalid GC time fraction"));
            }
            parse(digits)? * 10_i64.pow((9 - digits.len()) as u32)
        }
        _ => return Err(GCError::new("invalid GC time")),
    };
    if offset.len() != 5 || !matches!(&offset[0..1], "+" | "-") {
        return Err(GCError::new("invalid GC time timezone"));
    }
    let offset_hours = parse(&offset[1..3])?;
    let offset_minutes = parse(&offset[3..5])?;
    if offset_hours > 23 || offset_minutes > 59 {
        return Err(GCError::new("invalid GC time timezone"));
    }
    let offset_seconds =
        (offset_hours * 60 + offset_minutes) * 60 * if &offset[0..1] == "+" { 1 } else { -1 };
    let unix_seconds = days_from_civil(year, month, day)
        .checked_mul(86_400)
        .and_then(|seconds| seconds.checked_add(hour * 3_600 + minute * 60 + second))
        .and_then(|seconds| seconds.checked_sub(offset_seconds))
        .ok_or_else(|| GCError::new("GC time overflow"))?;
    let unix_nanos = i128::from(unix_seconds)
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(i128::from(fraction_nanos)))
        .ok_or_else(|| GCError::new("GC time overflow"))?;
    let absolute_nanos = unix_nanos.unsigned_abs();
    let duration = Duration::new(
        u64::try_from(absolute_nanos / 1_000_000_000)
            .map_err(|_| GCError::new("GC time overflow"))?,
        (absolute_nanos % 1_000_000_000) as u32,
    );
    if unix_nanos >= 0 {
        Ok(SystemTime::UNIX_EPOCH + duration)
    } else {
        SystemTime::UNIX_EPOCH
            .checked_sub(duration)
            .ok_or_else(|| GCError::new("GC time overflow"))
    }
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

/// 将时长格式化为可持久化字符串。
pub(crate) fn format_duration(duration: Duration) -> String {
    let nanos = duration.as_nanos();
    if nanos == 0 {
        return "0s".to_owned();
    }
    if nanos < 1_000_000_000 {
        let (unit_nanos, suffix) = if nanos < 1_000 {
            (1, "ns")
        } else if nanos < 1_000_000 {
            (1_000, "µs")
        } else {
            (1_000_000, "ms")
        };
        return format_decimal_duration(nanos, unit_nanos, suffix);
    }

    let mut seconds = duration.as_secs();
    let hours = seconds / 3_600;
    seconds %= 3_600;
    let minutes = seconds / 60;
    seconds %= 60;
    let mut result = String::new();
    if hours > 0 {
        result.push_str(&format!("{hours}h"));
    }
    if hours > 0 || minutes > 0 {
        result.push_str(&format!("{minutes}m"));
    }
    result.push_str(&seconds.to_string());
    if duration.subsec_nanos() != 0 {
        result.push('.');
        result.push_str(format!("{:09}", duration.subsec_nanos()).trim_end_matches('0'));
    }
    result.push('s');
    result
}

fn format_decimal_duration(nanos: u128, unit_nanos: u128, suffix: &str) -> String {
    let whole = nanos / unit_nanos;
    let remainder = nanos % unit_nanos;
    if remainder == 0 {
        return format!("{whole}{suffix}");
    }
    let width = unit_nanos.ilog10() as usize;
    let fraction = format!("{remainder:0width$}");
    format!("{whole}.{}{suffix}", fraction.trim_end_matches('0'))
}

/// 解析系统表中的时长字符串。
pub(crate) fn parse_duration(value: &str) -> GCResult<Duration> {
    // Match Go's time.ParseDuration grammar: a duration is a sequence of
    // decimal number/unit pairs (for example, "1h30m" or "1.5s"). Store the
    // result as nanoseconds so fractional values are truncated at the same
    // precision as Go's time.Duration.
    if value.is_empty() {
        return Err(GCError::new("invalid duration"));
    }
    let mut rest = value;
    if let Some(sign) = rest.chars().next() {
        if sign == '-' {
            return Err(GCError::new("negative durations are not supported"));
        }
        if sign == '+' {
            rest = &rest[sign.len_utf8()..];
        }
    }
    if rest.is_empty() {
        return Err(GCError::new(format!("invalid duration {value:?}")));
    }

    let mut total_nanos = 0u128;
    while !rest.is_empty() {
        let number_end = rest
            .char_indices()
            .take_while(|(_, character)| character.is_ascii_digit() || *character == '.')
            .last()
            .map_or(0, |(index, character)| index + character.len_utf8());
        if number_end == 0 {
            return Err(GCError::new(format!("invalid duration {value:?}")));
        }
        let number = &rest[..number_end];
        let (integer, fraction) = number.split_once('.').unwrap_or((number, ""));
        if integer.is_empty() && fraction.is_empty()
            || number.matches('.').count() > 1
            || !integer.chars().all(|character| character.is_ascii_digit())
            || !fraction.chars().all(|character| character.is_ascii_digit())
        {
            return Err(GCError::new(format!("invalid duration {value:?}")));
        }

        let units = &rest[number_end..];
        let (unit, nanos_per_unit) = [
            ("ns", 1u128),
            ("us", 1_000),
            ("µs", 1_000),
            ("μs", 1_000),
            ("ms", 1_000_000),
            ("s", 1_000_000_000),
            ("m", 60_000_000_000),
            ("h", 3_600_000_000_000),
        ]
        .into_iter()
        .find(|(unit, _)| units.starts_with(unit))
        .ok_or_else(|| GCError::new(format!("invalid duration {value:?}")))?;

        let whole = if integer.is_empty() {
            0
        } else {
            integer
                .parse::<u128>()
                .map_err(|_| GCError::new("duration overflow"))?
        };
        let whole_nanos = whole
            .checked_mul(nanos_per_unit)
            .ok_or_else(|| GCError::new("duration overflow"))?;
        let fraction_nanos = if fraction.is_empty() {
            0
        } else {
            let precision = fraction.len().min(9);
            let fraction_value = fraction[..precision]
                .parse::<u128>()
                .map_err(|_| GCError::new(format!("invalid duration {value:?}")))?;
            fraction_value
                .checked_mul(nanos_per_unit)
                .and_then(|nanos| nanos.checked_div(10u128.pow(precision as u32)))
                .ok_or_else(|| GCError::new("duration overflow"))?
        };
        total_nanos = total_nanos
            .checked_add(whole_nanos)
            .and_then(|nanos| nanos.checked_add(fraction_nanos))
            .ok_or_else(|| GCError::new("duration overflow"))?;
        rest = &units[unit.len()..];
    }

    u64::try_from(total_nanos)
        .map(Duration::from_nanos)
        .map_err(|_| GCError::new("duration overflow"))
}
