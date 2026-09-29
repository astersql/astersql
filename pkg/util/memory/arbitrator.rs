// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 内存仲裁器（MemArbitrator）核心类型与配额逻辑。
//
// 在进程内按软/硬限制、工作模式与任务优先级协调内存分配；
// 超额时可通过 ArbitrationContext 停止查询（OOM 风险杀除或取消）。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::sqlkiller::KillEventChan;

/// 仲裁器允许的最大硬限制（字节）。
pub const DefMaxLimit: i64 = 5_000_000_000_000_000;
/// 堆回收挂起检查的最长等待窗口。
pub const defHeapReclaimCheckMaxDuration: Duration = Duration::from_secs(5);
const DEFAULT_AWAIT_FREE_SHARDS: usize = 256;
const DIGEST_PROFILE_TIME_ALIGN_SEC: i64 = 30;
const DIGEST_PROFILE_WINDOW_SLOTS: usize = 3;
const DIGEST_PROFILE_ACTIVE_SLOTS: i64 = 2;
const POOL_CONSUMPTION_BUCKETS: usize = 500;
const CONTEXT_CACHE_IDLE_TIMEOUT_SEC: i64 = 10 * 60;
const MEM_MAGNIF_PROFILE_ALIGN_SEC: i64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 一次配额仲裁的结果。
pub struct ArbitrateResult(pub i32);
/// 配额申请成功。
pub const ArbitrateOk: ArbitrateResult = ArbitrateResult(0);
/// 配额申请失败或被取消。
pub const ArbitrateFail: ArbitrateResult = ArbitrateResult(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 仲裁优先级：LOW / MEDIUM / HIGH。
pub struct ArbitrationPriority(pub i32);
/// 低优先级任务。
pub const ArbitrationPriorityLow: ArbitrationPriority = ArbitrationPriority(0);
/// 中优先级任务。
pub const ArbitrationPriorityMedium: ArbitrationPriority = ArbitrationPriority(1);
/// 高优先级任务。
pub const ArbitrationPriorityHigh: ArbitrationPriority = ArbitrationPriority(2);
/// 任务计数数组中 wait-averse（不愿等待）桶的下标。
pub const ArbitrationWaitAverse: usize = 3;

impl ArbitrationPriority {
    /// 返回与 Go 一致的优先级名称字符串。
    pub fn String(self) -> String {
        match self.0 {
            0 => "LOW",
            1 => "MEDIUM",
            2 => "HIGH",
            _ => panic!("invalid arbitration priority: {}", self.0),
        }
        .to_owned()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 仲裁工作模式：standard / priority / disable。
pub struct ArbitratorWorkMode(pub i32);
/// 标准模式。
pub const ArbitratorModeStandard: ArbitratorWorkMode = ArbitratorWorkMode(0);
/// 按优先级抢占/取消的模式。
pub const ArbitratorModePriority: ArbitratorWorkMode = ArbitratorWorkMode(1);
/// 禁用仲裁。
pub const ArbitratorModeDisable: ArbitratorWorkMode = ArbitratorWorkMode(2);

impl ArbitratorWorkMode {
    pub const Standard: Self = ArbitratorModeStandard;
    pub const Priority: Self = ArbitratorModePriority;
    pub const Disable: Self = ArbitratorModeDisable;

    pub fn from_text(value: &str) -> Self {
        match value {
            "standard" => Self::Standard,
            "priority" => Self::Priority,
            _ => Self::Disable,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self.0 {
            0 => "standard",
            1 => "priority",
            _ => "disable",
        }
    }

    /// 返回与 Go 一致的工作模式名称。
    pub fn String(self) -> String {
        match self.0 {
            0 => "standard",
            1 => "priority",
            2 => "disable",
            _ => panic!("invalid arbitrator work mode: {}", self.0),
        }
        .to_owned()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 软限制模式：关闭 / 指定值 / 自动。
pub struct SoftLimitMode(pub i32);
/// 不使用指定软限制（回落到 oomRisk）。
pub const SoftLimitModeDisable: SoftLimitMode = SoftLimitMode(0);
/// 使用指定字节数或比例作为软限制。
pub const SoftLimitModeSpecified: SoftLimitMode = SoftLimitMode(1);
/// 自动软限制模式。
pub const SoftLimitModeAuto: SoftLimitMode = SoftLimitMode(2);

impl SoftLimitMode {
    pub const Disable: Self = SoftLimitModeDisable;
    pub const Specified: Self = SoftLimitModeSpecified;
    pub const Auto: Self = SoftLimitModeAuto;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 停止查询的原因编码。
pub struct ArbitratorStopReason(pub isize);
/// OOM 风险：直接 KILL。
pub const ArbitratorOOMRiskKill: ArbitratorStopReason = ArbitratorStopReason(0);
/// 配额不足且 wait-averse：CANCEL。
pub const ArbitratorWaitAverseCancel: ArbitratorStopReason = ArbitratorStopReason(1);
/// 标准模式下配额不足：CANCEL。
pub const ArbitratorStandardCancel: ArbitratorStopReason = ArbitratorStopReason(2);
/// 优先级模式下配额不足：CANCEL。
pub const ArbitratorPriorityCancel: ArbitratorStopReason = ArbitratorStopReason(3);

impl ArbitratorStopReason {
    /// 返回停止原因文案；未知码为 UNKNOWN。
    pub fn String(self) -> String {
        match self.0 {
            0 => "KILL(out-of-memory)",
            1 => "CANCEL(out-of-quota & wait-averse)",
            2 => "CANCEL(out-of-quota & standard-mode)",
            3 => "CANCEL(out-of-quota & priority-mode)",
            _ => "UNKNOWN",
        }
        .to_owned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// ConcurrentBudget 超额时的错误标记。
pub struct BudgetExhausted;

/// 无锁并发配额桶：容量、已用、最近使用 Unix 秒。
pub struct ConcurrentBudget {
    capacity: AtomicI64,
    used: AtomicI64,
    last_used_time_sec: AtomicI64,
}

impl ConcurrentBudget {
    /// 以非负容量构造预算。
    pub fn new(capacity: i64) -> Self {
        Self {
            capacity: AtomicI64::new(capacity.max(0)),
            used: AtomicI64::new(0),
            last_used_time_sec: AtomicI64::new(0),
        }
    }

    /// 增减已用配额；正向超额时返回 `BudgetExhausted`（仍已加上 req）。
    pub fn ConsumeQuota(&self, utime_sec: i64, req: i64) -> Result<(), BudgetExhausted> {
        if req > 0 {
            self.last_used_time_sec.store(utime_sec, Ordering::Release);
        }
        let used = self.used.fetch_add(req, Ordering::AcqRel) + req;
        if req > 0 && used > self.capacity.load(Ordering::Acquire) {
            return Err(BudgetExhausted);
        }
        Ok(())
    }

    /// 将容量提高到指定目标，且不得低于当前用量。
    pub fn Reserve(&self, new_capacity: i64) {
        let _ = self
            .capacity
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |capacity| {
                Some(capacity.max(new_capacity).max(self.used()))
            });
    }

    /// 当前已用字节。
    pub fn used(&self) -> i64 {
        self.used.load(Ordering::Acquire)
    }
    /// 当前容量。
    pub fn capacity(&self) -> i64 {
        self.capacity.load(Ordering::Acquire)
    }
    /// 最近一次正向消耗的 Unix 秒。
    pub fn last_used_time_sec(&self) -> i64 {
        self.last_used_time_sec.load(Ordering::Acquire)
    }

    /// 停止预算并返回原容量；与 Go Stop 的一次性释放语义一致。
    pub fn Stop(&self) -> i64 {
        self.used.store(0, Ordering::Release);
        self.capacity.swap(0, Ordering::AcqRel)
    }

    fn release_capacity(&self, amount: i64) -> i64 {
        let mut removed = 0;
        let _ = self
            .capacity
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |capacity| {
                removed = capacity.max(0).min(amount.max(0));
                Some(capacity - removed)
            });
        removed
    }
}

impl Default for ConcurrentBudget {
    fn default() -> Self {
        Self::new(0)
    }
}

/// await-free 分片预算，同时跟踪配额与实际 heap-inuse。
pub struct TrackedConcurrentBudget {
    budget: ConcurrentBudget,
    heap_inuse: AtomicI64,
}

impl TrackedConcurrentBudget {
    fn new(capacity: i64) -> Self {
        Self {
            budget: ConcurrentBudget::new(capacity),
            heap_inuse: AtomicI64::new(0),
        }
    }

    pub fn ConsumeQuota(&self, utime_sec: i64, req: i64) -> Result<(), BudgetExhausted> {
        self.budget.ConsumeQuota(utime_sec, req)
    }

    pub fn ReportHeapInuse(&self, req: i64) {
        self.heap_inuse.fetch_add(req, Ordering::AcqRel);
    }

    pub fn used(&self) -> i64 {
        self.budget.used()
    }

    pub fn heap_inuse(&self) -> i64 {
        self.heap_inuse.load(Ordering::Acquire)
    }

    pub fn capacity(&self) -> i64 {
        self.budget.capacity()
    }

    pub fn Reserve(&self, capacity: i64) {
        self.budget.Reserve(capacity);
    }

    fn release_capacity(&self, amount: i64) -> i64 {
        self.budget.release_capacity(amount)
    }
}

/// 一次仲裁上下文对应的 root pool 用量与实际堆占用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemUsage {
    pub RootPoolUsed: i64,
    pub HeapInuse: i64,
}

/// 仲裁上下文回调：停止、查询堆占用、结束。
pub trait ArbitrateHelper: Send + Sync {
    fn Stop(&self, reason: ArbitratorStopReason) -> bool;
    fn HeapInuse(&self) -> i64;
    fn MemUsage(&self) -> MemUsage {
        MemUsage {
            RootPoolUsed: 0,
            HeapInuse: self.HeapInuse(),
        }
    }
    fn Finish(&self);
    fn Done(&self) -> CancelReceiver {
        CancelReceiver::none()
    }
}

/// 可克隆的一次性取消接收端；默认值表示没有取消通道。
#[derive(Clone, Default)]
pub struct CancelReceiver(Option<KillEventChan>);

impl CancelReceiver {
    pub fn none() -> Self {
        Self(None)
    }

    pub fn from_kill_event(event: KillEventChan) -> Self {
        Self(Some(event))
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.as_ref().is_some_and(KillEventChan::is_closed)
    }
}

/// 单次仲裁会话上下文：优先级、wait-averse、特权偏好与停止标志。
pub struct ArbitrationContext {
    helper: RwLock<Option<Arc<dyn ArbitrateHelper>>>,
    cancel_ch: CancelReceiver,
    stopped: AtomicBool,
    pub memPriority: ArbitrationPriority,
    pub waitAverse: bool,
    pub preferPrivilege: bool,
}

impl ArbitrationContext {
    /// 上下文是否仍可用（未停止）。
    pub fn available(&self) -> bool {
        self.helper
            .read()
            .expect("arbitration helper lock poisoned")
            .as_ref()
            .is_some_and(|helper| !helper.Done().is_cancelled())
            && !self.stopped.load(Ordering::Acquire)
            && !self.cancel_ch.is_cancelled()
    }

    /// 首次停止时通知 helper；重复调用无效。
    pub fn stop(&self, reason: ArbitratorStopReason) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            if let Some(helper) = self
                .helper
                .read()
                .expect("arbitration helper lock poisoned")
                .as_ref()
            {
                helper.Stop(reason);
            }
        }
    }

    pub fn set_helper(&self, helper: Arc<dyn ArbitrateHelper>) {
        *self
            .helper
            .write()
            .expect("arbitration helper lock poisoned") = Some(helper);
    }
}

/// 构造仲裁上下文，保留 Go 的可空 helper 与取消通道语义。
pub fn NewArbitrationContext(
    cancel_ch: CancelReceiver,
    helper: Option<Arc<dyn ArbitrateHelper>>,
    mem_priority: ArbitrationPriority,
    wait_averse: bool,
    prefer_privilege: bool,
) -> Arc<ArbitrationContext> {
    Arc::new(ArbitrationContext {
        helper: RwLock::new(helper),
        cancel_ch,
        stopped: AtomicBool::new(false),
        memPriority: mem_priority,
        waitAverse: wait_averse,
        preferPrivilege: prefer_privilege,
    })
}

/// 按优先级与 wait-averse 分桶的任务计数 `[low, mid, high, waitAverse]`。
pub type NumByPattern = [i64; 4];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// 成功/失败计数对。
pub struct PairSuccessFail {
    pub Succ: i64,
    pub Fail: i64,
}

/// 按 LOW / MEDIUM / HIGH 顺序统计的任务数。
pub type NumByPriority = [i64; 3];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AwaitFreePoolExecMetrics {
    pub Succ: i64,
    pub Fail: i64,
    pub Shrink: i64,
    pub ForceShrink: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExecMetricsAction {
    pub GC: i64,
    pub UpdateRuntimeMemStats: i64,
    pub RecordMemState: PairSuccessFail,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExecMetricsRisk {
    pub Mem: i64,
    pub OOM: i64,
    pub OOMKill: NumByPriority,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExecMetricsCancel {
    pub StandardMode: i64,
    pub PriorityMode: NumByPriority,
    pub WaitAverse: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExecMetricsTask {
    pub Succ: i64,
    pub Fail: i64,
    pub SuccByPriority: NumByPriority,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Go `execMetricsCounter` 的公开快照。
pub struct ExecMetricsCounter {
    pub Task: ExecMetricsTask,
    pub Cancel: ExecMetricsCancel,
    pub AwaitFree: AwaitFreePoolExecMetrics,
    pub Action: ExecMetricsAction,
    pub Risk: ExecMetricsRisk,
    pub ShrinkDigest: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Go memStats 在 Rust 中的字段映射。
pub struct ArbitratorRuntimeStats {
    pub heap_alloc: i64,
    pub heap_inuse: i64,
    pub mem_off_heap: i64,
    pub total_free: i64,
    pub last_gc_unix_nano: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolAllocProfile {
    pub SmallPoolLimit: i64,
    pub PoolAllocUnit: i64,
    pub MaxPoolAllocUnit: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RootPoolHandle {
    uid: u64,
}

struct RootPoolEntry {
    quota: i64,
    max_quota: i64,
    context: Option<Arc<ArbitrationContext>>,
    running: bool,
}

struct PendingRequest {
    handle: RootPoolHandle,
    request: i64,
    context: Arc<ArbitrationContext>,
    result: Arc<(Mutex<Option<ArbitrateResult>>, Condvar)>,
    reclaim_started: Option<Instant>,
}

#[derive(Clone, Copy)]
struct DigestProfile {
    max_value: i64,
    last_fetch_unix_sec: i64,
    timed_max: [(i64, i64); DIGEST_PROFILE_WINDOW_SLOTS],
}

struct BufferProfile {
    timed_max: [(i64, i64); DIGEST_PROFILE_WINDOW_SLOTS],
}

#[derive(Clone, Copy, Default)]
struct MemMagnifProfile {
    time_slot: i64,
    heap: i64,
    quota: i64,
    ratio: i64,
}

struct TimedConsumption {
    time_slot: i64,
    counts: [u32; POOL_CONSUMPTION_BUCKETS],
    total: u64,
}

#[derive(Clone, Copy)]
struct SoftLimitState {
    size: i64,
    specified_size: i64,
    specified_ratio: f64,
    mode: SoftLimitMode,
}

/// 进程内内存仲裁状态：硬限制、已分配、堆占用、软限制与风险标志。
pub struct MemArbitrator {
    limit: AtomicI64,
    allocated: AtomicI64,
    heap_alloc: AtomicI64,
    heap_inuse: AtomicI64,
    out_of_control: AtomicI64,
    reserved_buffer: AtomicI64,
    buffer_profile: Mutex<BufferProfile>,
    pool_consumption: Mutex<[TimedConsumption; DIGEST_PROFILE_WINDOW_SLOTS]>,
    pool_medium_quota: AtomicI64,
    mem_magnif: AtomicI64,
    mem_magnif_profiles: Mutex<[MemMagnifProfile; 2]>,
    last_blocked: Mutex<(i64, i64)>,
    soft_limit: Mutex<SoftLimitState>,
    mode: AtomicI32,
    tasks: Mutex<NumByPattern>,
    roots: Mutex<HashMap<u64, RootPoolEntry>>,
    under_kill: Mutex<HashMap<u64, (i64, Instant)>>,
    under_cancel: Mutex<HashMap<u64, (i64, Instant)>>,
    context_cache: Mutex<HashMap<u64, i64>>,
    pending: Mutex<VecDeque<PendingRequest>>,
    round_mutex: Mutex<()>,
    waiting_alloc: AtomicI64,
    digest_profiles: Mutex<HashMap<u64, DigestProfile>>,
    digest_profile_limit: AtomicI64,
    await_free: Vec<Arc<TrackedConcurrentBudget>>,
    at_mem_risk: AtomicBool,
    at_oom_risk: AtomicBool,
    mem_risk_since: Mutex<Option<Instant>>,
    unix_time_sec: AtomicI64,
    runtime_stats: Mutex<ArbitratorRuntimeStats>,
    exec_metrics: Mutex<ExecMetricsCounter>,
    running: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Default for MemArbitrator {
    fn default() -> Self {
        NewMemArbitrator(DefMaxLimit)
    }
}

/// 构造仲裁器；非正 limit 使用 DefMaxLimit，并裁剪到上限。
pub fn NewMemArbitrator(limit: i64) -> MemArbitrator {
    let limit = if limit <= 0 {
        DefMaxLimit
    } else {
        limit.min(DefMaxLimit)
    };
    let result = MemArbitrator {
        limit: AtomicI64::new(limit),
        allocated: AtomicI64::new(0),
        heap_alloc: AtomicI64::new(0),
        heap_inuse: AtomicI64::new(0),
        out_of_control: AtomicI64::new(0),
        reserved_buffer: AtomicI64::new(0),
        buffer_profile: Mutex::new(BufferProfile {
            timed_max: [(i64::MIN, 0); DIGEST_PROFILE_WINDOW_SLOTS],
        }),
        pool_consumption: Mutex::new(std::array::from_fn(|_| TimedConsumption {
            time_slot: i64::MIN,
            counts: [0; POOL_CONSUMPTION_BUCKETS],
            total: 0,
        })),
        pool_medium_quota: AtomicI64::new(0),
        mem_magnif: AtomicI64::new(0),
        mem_magnif_profiles: Mutex::new([MemMagnifProfile::default(); 2]),
        last_blocked: Mutex::new((0, 0)),
        soft_limit: Mutex::new(SoftLimitState {
            size: (limit as f64 * 0.95) as i64,
            specified_size: 0,
            specified_ratio: 0.0,
            mode: SoftLimitModeDisable,
        }),
        mode: AtomicI32::new(ArbitratorModeDisable.0),
        tasks: Mutex::new([0; 4]),
        roots: Mutex::new(HashMap::new()),
        under_kill: Mutex::new(HashMap::new()),
        under_cancel: Mutex::new(HashMap::new()),
        context_cache: Mutex::new(HashMap::new()),
        pending: Mutex::new(VecDeque::new()),
        round_mutex: Mutex::new(()),
        waiting_alloc: AtomicI64::new(0),
        digest_profiles: Mutex::new(HashMap::new()),
        digest_profile_limit: AtomicI64::new(40_000),
        await_free: (0..DEFAULT_AWAIT_FREE_SHARDS)
            .map(|_| Arc::new(TrackedConcurrentBudget::new(0)))
            .collect(),
        at_mem_risk: AtomicBool::new(false),
        at_oom_risk: AtomicBool::new(false),
        mem_risk_since: Mutex::new(None),
        unix_time_sec: AtomicI64::new(now_unix_sec()),
        runtime_stats: Mutex::new(ArbitratorRuntimeStats::default()),
        exec_metrics: Mutex::new(ExecMetricsCounter::default()),
        running: AtomicBool::new(false),
        worker: Mutex::new(None),
    };
    result
}

impl MemArbitrator {
    /// 按模式重算 soft_limit（指定值/比例，或默认 oomRisk）。
    fn adjust_soft_limit(&self) {
        let limit = self.Limit();
        let mut state = self.soft_limit.lock().expect("soft limit lock poisoned");
        state.size = if state.mode == SoftLimitModeSpecified {
            if state.specified_size > 0 {
                state.specified_size.min(limit)
            } else {
                ((limit as f64 * state.specified_ratio) as i64).min(limit)
            }
        } else {
            self.oomRisk()
        };
    }

    /// 设置软限制模式及指定参数。
    pub fn SetSoftLimit(&self, size: i64, ratio: f64, mode: SoftLimitMode) {
        {
            let mut state = self.soft_limit.lock().expect("soft limit lock poisoned");
            state.mode = mode;
            if mode == SoftLimitModeSpecified {
                state.specified_size = size;
                state.specified_ratio = ratio;
            }
        }
        self.adjust_soft_limit();
    }

    /// 当前软限制字节数。
    pub fn SoftLimit(&self) -> i64 {
        self.soft_limit
            .lock()
            .expect("soft limit lock poisoned")
            .size
    }

    /// 返回软限制原始配置，供全局配置测试与诊断使用。
    pub fn SoftLimitConfig(&self) -> (i64, f64, SoftLimitMode) {
        let state = self.soft_limit.lock().expect("soft limit lock poisoned");
        (state.specified_size, state.specified_ratio, state.mode)
    }

    /// 当前硬限制。
    pub fn Limit(&self) -> i64 {
        self.limit.load(Ordering::Acquire)
    }

    /// 内存风险阈值：硬限制的 90%。
    pub fn memRisk(&self) -> i64 {
        (self.Limit() as f64 * 0.90) as i64
    }

    /// OOM 风险阈值：硬限制的 95%。
    pub fn oomRisk(&self) -> i64 {
        (self.Limit() as f64 * 0.95) as i64
    }

    /// 更新硬限制；无效或未变化返回 false。
    pub fn SetLimit(&self, value: u64) -> bool {
        let new_limit = (value.min(DefMaxLimit as u64)) as i64;
        if new_limit <= 0 {
            return false;
        }
        let old = self.limit.swap(new_limit, Ordering::AcqRel);
        if old == new_limit {
            return false;
        }
        self.adjust_soft_limit();
        true
    }

    /// 尝试占用配额；不足则失败且不修改已分配量。
    pub fn allocate(&self, request: i64) -> bool {
        if request <= 0 {
            return true;
        }
        loop {
            let allocated = self.allocated.load(Ordering::Acquire);
            if request
                > self.Limit()
                    - self.reservedBuffer()
                    - self.out_of_control.load(Ordering::Acquire)
                    - allocated
            {
                return false;
            }
            if self
                .allocated
                .compare_exchange(
                    allocated,
                    allocated + request,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return true;
            }
        }
    }

    /// 归还配额，已分配量不低于 0。
    pub fn release(&self, size: i64) {
        let size = size.max(0);
        let _ = self
            .allocated
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |allocated| {
                Some((allocated - size).max(0))
            });
    }

    /// 已由仲裁器记账的分配量。
    pub fn Allocated(&self) -> i64 {
        self.allocated.load(Ordering::Acquire)
    }

    /// 失控（未纳入配额）内存估计。
    pub fn OutOfControl(&self) -> i64 {
        self.out_of_control.load(Ordering::Acquire)
    }

    /// Runtime counters used by the heap-profile metadata snapshot.
    pub fn HeapProfileCounters(&self) -> (i64, i64, i64) {
        let stats = *self
            .runtime_stats
            .lock()
            .expect("runtime stats lock poisoned");
        (
            stats.heap_alloc.max(0),
            stats.heap_inuse.max(0),
            self.heap_inuse.load(Ordering::Acquire),
        )
    }

    pub fn MemMagnif(&self) -> i64 {
        self.mem_magnif.load(Ordering::Acquire)
    }

    /// Restore the last successfully persisted tuning values at startup.
    pub(crate) fn RestoreRuntimeMemState(&self, magnif: i64, pool_medium_cap: i64) {
        self.mem_magnif.store(magnif, Ordering::Release);
        self.pool_medium_quota
            .store(pool_medium_cap, Ordering::Release);
    }

    fn update_mem_magnification(
        &self,
        now_sec: i64,
        last_gc_sec: i64,
        last_gc_heap: i64,
        blocked_sec: i64,
        blocked_quota: i64,
    ) -> bool {
        let slot = now_sec.div_euclid(MEM_MAGNIF_PROFILE_ALIGN_SEC);
        let index = slot.rem_euclid(2) as usize;
        let mut profiles = self
            .mem_magnif_profiles
            .lock()
            .expect("magnification profile lock poisoned");
        let mut changed = false;
        if profiles[index].time_slot < slot {
            let previous_slot = slot - 1;
            let previous = previous_slot.rem_euclid(2) as usize;
            if profiles[previous].time_slot == previous_slot && profiles[previous].quota > 0 {
                profiles[previous].ratio =
                    crate::calcRatio(profiles[previous].heap, profiles[previous].quota);
            }
            let mut safe_ratio = 0;
            for age in [2, 1] {
                let target_slot = slot - age;
                let profile = profiles[target_slot.rem_euclid(2) as usize];
                if profile.time_slot != target_slot || profile.heap >= self.oomRisk() {
                    safe_ratio = 0;
                    break;
                }
                if profile.ratio <= 0 {
                    break;
                }
                safe_ratio = safe_ratio.max(profile.ratio);
            }
            let old = self.MemMagnif();
            if safe_ratio > 0 && old > 0 && safe_ratio < old - 10 {
                let next = (old + safe_ratio) / 2;
                self.mem_magnif
                    .store(if next <= 1_000 { 0 } else { next }, Ordering::Release);
                changed = true;
            }
            profiles[index] = MemMagnifProfile {
                time_slot: slot,
                ..Default::default()
            };
        }
        if profiles[index].time_slot == slot {
            if last_gc_sec.div_euclid(MEM_MAGNIF_PROFILE_ALIGN_SEC) == slot {
                profiles[index].heap = profiles[index].heap.max(last_gc_heap);
            }
            if blocked_sec.div_euclid(MEM_MAGNIF_PROFILE_ALIGN_SEC) == slot {
                profiles[index].quota = profiles[index].quota.max(blocked_quota);
            }
        }
        changed
    }

    #[cfg(test)]
    pub fn SetMemMagnifForTest(&self, ratio: i64) {
        self.mem_magnif.store(ratio, Ordering::Release);
    }

    #[cfg(test)]
    pub fn UpdateMemMagnificationForTest(
        &self,
        now_sec: i64,
        last_gc_sec: i64,
        last_gc_heap: i64,
        blocked_sec: i64,
        blocked_quota: i64,
    ) -> bool {
        self.update_mem_magnification(
            now_sec,
            last_gc_sec,
            last_gc_heap,
            blocked_sec,
            blocked_quota,
        )
    }

    #[cfg(test)]
    pub fn MemMagnifForTest(&self) -> i64 {
        self.mem_magnif.load(Ordering::Acquire)
    }

    /// 相对堆占用的可用量。
    pub fn heapAvailable(&self) -> i64 {
        self.Limit() - self.reservedBuffer() - self.heap_alloc.load(Ordering::Acquire)
    }

    /// 相对配额记账的可用量。
    pub fn quotaAvailable(&self) -> i64 {
        self.Limit() - self.reservedBuffer() - self.OutOfControl() - self.Allocated()
    }

    /// 堆可用与配额可用的较小值。
    pub fn available(&self) -> i64 {
        self.heapAvailable().min(self.quotaAvailable())
    }

    fn cleanup_context_cache(&self) {
        let deadline = self.approxUnixTimeSec() - CONTEXT_CACHE_IDLE_TIMEOUT_SEC;
        self.context_cache
            .lock()
            .expect("context cache lock poisoned")
            .retain(|_, idle_time| *idle_time == 0 || *idle_time > deadline);
    }

    #[cfg(test)]
    pub fn ContextCacheNumForTest(&self) -> usize {
        self.context_cache
            .lock()
            .expect("context cache lock poisoned")
            .len()
    }

    fn reservedBuffer(&self) -> i64 {
        if self.WorkMode() == ArbitratorModePriority {
            self.reserved_buffer.load(Ordering::Acquire)
        } else {
            0
        }
    }

    #[cfg(test)]
    pub(crate) fn ReservedBufferForTest(&self) -> i64 {
        self.reservedBuffer()
    }

    /// Records a large session's observed usage when it first enters the root pool.
    pub fn TryToUpdateBuffer(&self, mem_consumed: i64, utime_sec: i64) {
        if self.WorkMode() != ArbitratorModePriority {
            return;
        }
        let time_slot = utime_sec / DIGEST_PROFILE_TIME_ALIGN_SEC;
        let mut profile = self
            .buffer_profile
            .lock()
            .expect("buffer profile lock poisoned");
        let slot = &mut profile.timed_max
            [time_slot.rem_euclid(DIGEST_PROFILE_WINDOW_SLOTS as i64) as usize];
        if slot.0 != time_slot {
            *slot = (time_slot, 0);
        }
        slot.1 = slot.1.max(mem_consumed.max(0));
        let max_recent = profile
            .timed_max
            .iter()
            .filter(|(slot_time, _)| {
                *slot_time > time_slot - DIGEST_PROFILE_ACTIVE_SLOTS && *slot_time <= time_slot
            })
            .map(|(_, value)| *value)
            .max()
            .unwrap_or(0);
        self.reserved_buffer.store(max_recent, Ordering::Release);
    }

    fn refresh_buffer_from_contexts(&self) {
        if self.WorkMode() != ArbitratorModePriority {
            return;
        }
        let cached_ids: Vec<_> = self
            .context_cache
            .lock()
            .expect("context cache lock poisoned")
            .keys()
            .copied()
            .collect();
        let contexts: Vec<_> = {
            let roots = self.roots.lock().expect("root pool lock poisoned");
            cached_ids
                .iter()
                .filter_map(|uid| roots.get(uid))
                .filter(|entry| entry.running)
                .filter_map(|entry| entry.context.clone())
                .collect()
        };
        let max_heap = contexts
            .iter()
            .filter_map(|context| {
                context
                    .helper
                    .read()
                    .expect("arbitration helper lock poisoned")
                    .as_ref()
                    .map(|helper| helper.MemUsage().HeapInuse)
            })
            .max()
            .unwrap_or(0)
            .max(0);
        let time_slot = self.approxUnixTimeSec() / DIGEST_PROFILE_TIME_ALIGN_SEC;
        let mut profile = self
            .buffer_profile
            .lock()
            .expect("buffer profile lock poisoned");
        let slot = &mut profile.timed_max
            [time_slot.rem_euclid(DIGEST_PROFILE_WINDOW_SLOTS as i64) as usize];
        if slot.0 != time_slot {
            *slot = (time_slot, 0);
        }
        slot.1 = slot.1.max(max_heap);
        let max_recent = profile
            .timed_max
            .iter()
            .filter(|(slot_time, _)| {
                *slot_time > time_slot - DIGEST_PROFILE_ACTIVE_SLOTS && *slot_time <= time_slot
            })
            .map(|(_, value)| *value)
            .max()
            .unwrap_or(0);
        self.reserved_buffer.store(max_recent, Ordering::Release);
    }

    fn record_mem_consumed(&self, consumed: i64) {
        let time_slot = self.approxUnixTimeSec() / DIGEST_PROFILE_TIME_ALIGN_SEC;
        let unit = self.PoolAllocProfile().PoolAllocUnit;
        let bucket = (consumed / unit).clamp(0, POOL_CONSUMPTION_BUCKETS as i64 - 1) as usize;
        let mut slots = self
            .pool_consumption
            .lock()
            .expect("pool consumption lock poisoned");
        let slot = &mut slots[time_slot.rem_euclid(DIGEST_PROFILE_WINDOW_SLOTS as i64) as usize];
        if slot.time_slot != time_slot {
            *slot = TimedConsumption {
                time_slot,
                counts: [0; POOL_CONSUMPTION_BUCKETS],
                total: 0,
            };
        }
        slot.counts[bucket] += 1;
        slot.total += 1;
    }

    fn refresh_pool_medium_quota(&self) {
        let time_slot = self.approxUnixTimeSec() / DIGEST_PROFILE_TIME_ALIGN_SEC;
        let slots = self
            .pool_consumption
            .lock()
            .expect("pool consumption lock poisoned");
        let recent: Vec<_> = slots
            .iter()
            .filter(|slot| {
                slot.time_slot > time_slot - DIGEST_PROFILE_ACTIVE_SLOTS
                    && slot.time_slot <= time_slot
            })
            .collect();
        let total: u64 = recent.iter().map(|slot| slot.total).sum();
        if total == 0 {
            return;
        }
        let target = ((total + 1) / 2).max(1);
        let mut count = 0;
        for bucket in 0..POOL_CONSUMPTION_BUCKETS {
            count += recent
                .iter()
                .map(|slot| slot.counts[bucket] as u64)
                .sum::<u64>();
            if count >= target {
                self.pool_medium_quota.store(
                    self.PoolAllocProfile().PoolAllocUnit * (bucket as i64 + 1),
                    Ordering::Release,
                );
                break;
            }
        }
    }

    /// 切换工作模式；返回是否真正发生变化。
    pub fn SetWorkMode(&self, mode: ArbitratorWorkMode) -> bool {
        if !matches!(mode.0, 0..=2) {
            return false;
        }
        self.mode.swap(mode.0, Ordering::AcqRel) != mode.0
    }

    /// 当前工作模式。
    pub fn WorkMode(&self) -> ArbitratorWorkMode {
        ArbitratorWorkMode(self.mode.load(Ordering::Acquire))
    }

    /// 返回分桶任务计数快照。
    pub fn TaskNumByPattern(&self) -> NumByPattern {
        *self.tasks.lock().expect("task counter lock poisoned")
    }

    /// 返回执行指标的一致快照。
    pub fn ExecMetrics(&self) -> ExecMetricsCounter {
        *self
            .exec_metrics
            .lock()
            .expect("execution metrics lock poisoned")
    }

    /// 登记一个任务到优先级桶，可选计入 wait-averse。
    pub fn record_task(&self, priority: ArbitrationPriority, wait_averse: bool) {
        let mut tasks = self.tasks.lock().expect("task counter lock poisoned");
        if let Some(value) = tasks.get_mut(priority.0 as usize) {
            *value += 1;
        }
        if wait_averse {
            tasks[ArbitrationWaitAverse] += 1;
        }
    }

    /// 堆占用是否达到 memRisk。
    pub fn AtMemRisk(&self) -> bool {
        self.at_mem_risk.load(Ordering::Acquire)
    }

    /// 堆占用是否达到 oomRisk。
    pub fn AtOOMRisk(&self) -> bool {
        self.at_oom_risk.load(Ordering::Acquire)
    }

    /// 更新运行时堆占用并刷新风险标志。
    pub fn set_runtime_heap(&self, heap_alloc: i64) {
        let heap_alloc = heap_alloc.max(0);
        self.heap_alloc.store(heap_alloc, Ordering::Release);
        self.at_mem_risk
            .store(heap_alloc >= self.memRisk(), Ordering::Release);
        self.at_oom_risk
            .store(heap_alloc >= self.oomRisk(), Ordering::Release);
    }

    /// 缓存近似 Unix 秒（避免高频系统调用）。
    pub fn setUnixTimeSec(&self, value: i64) {
        self.unix_time_sec.store(value, Ordering::Release);
    }

    /// 读取缓存的近似 Unix 秒。
    pub fn approxUnixTimeSec(&self) -> i64 {
        self.unix_time_sec.load(Ordering::Acquire)
    }

    pub fn HandleRuntimeStats(&self, stats: ArbitratorRuntimeStats) {
        let mem_inuse = stats.heap_inuse.saturating_add(stats.mem_off_heap).max(0);
        self.heap_alloc
            .store(stats.heap_alloc.max(0), Ordering::Release);
        self.heap_inuse.store(mem_inuse, Ordering::Release);
        *self
            .runtime_stats
            .lock()
            .expect("runtime stats lock poisoned") = stats;
        self.update_tracked_heap_avoidance(stats);
        self.setUnixTimeSec(now_unix_sec());

        let was_at_mem_risk = self.at_mem_risk.load(Ordering::Acquire);
        let at_mem_risk = if was_at_mem_risk {
            mem_inuse >= self.oomRisk() || stats.heap_alloc >= self.memRisk()
        } else {
            mem_inuse >= self.oomRisk()
        };
        let hard_oom_risk = mem_inuse > self.Limit();
        let at_oom_risk = {
            let mut since = self
                .mem_risk_since
                .lock()
                .expect("memory risk lock poisoned");
            if mem_inuse < self.oomRisk() {
                *since = None;
                false
            } else if hard_oom_risk {
                *since = Some(Instant::now());
                true
            } else if let Some(started) = *since {
                started.elapsed() >= Duration::from_secs(1)
            } else {
                *since = Some(Instant::now());
                false
            }
        };
        {
            let mut metrics = self
                .exec_metrics
                .lock()
                .expect("execution metrics lock poisoned");
            metrics.Action.UpdateRuntimeMemStats += 1;
            if at_mem_risk && !self.at_mem_risk.load(Ordering::Acquire) {
                metrics.Risk.Mem += 1;
            }
            if at_oom_risk && !self.at_oom_risk.load(Ordering::Acquire) {
                metrics.Risk.OOM += 1;
            }
        }
        self.at_mem_risk.store(at_mem_risk, Ordering::Release);
        self.at_oom_risk.store(at_oom_risk, Ordering::Release);

        if at_mem_risk && !was_at_mem_risk && self.SoftLimitConfig().2 == SoftLimitModeAuto {
            let quota = self.Allocated();
            if quota > 0 && stats.heap_alloc > quota {
                let magnif = crate::calcRatio(stats.heap_alloc, quota)
                    .saturating_add(100)
                    .max(self.mem_magnif.load(Ordering::Acquire))
                    .min(10_000);
                self.mem_magnif.store(magnif, Ordering::Release);
                self.update_tracked_heap_avoidance(stats);
            }
        }

        if at_oom_risk {
            self.cancel_for_oom();
        }
    }

    pub fn RuntimeStats(&self) -> ArbitratorRuntimeStats {
        *self
            .runtime_stats
            .lock()
            .expect("runtime stats lock poisoned")
    }

    fn update_tracked_heap_avoidance(&self, stats: ArbitratorRuntimeStats) {
        let cached_ids: Vec<_> = self
            .context_cache
            .lock()
            .expect("context cache lock poisoned")
            .keys()
            .copied()
            .collect();
        let entries: Vec<_> = {
            let roots = self.roots.lock().expect("root pool lock poisoned");
            cached_ids
                .iter()
                .filter_map(|uid| roots.get(uid))
                .filter(|entry| entry.running)
                .filter_map(|entry| entry.context.clone().map(|context| (entry.quota, context)))
                .collect()
        };
        let root_tracked = entries.into_iter().fold(0_i64, |total, (quota, context)| {
            let root_used = context
                .helper
                .read()
                .expect("arbitration helper lock poisoned")
                .as_ref()
                .map_or(0, |helper| helper.MemUsage().RootPoolUsed);
            total.saturating_add(root_used.max(0).min(quota.max(0)))
        });
        let (await_free_cap, await_free_tracked) =
            self.await_free
                .iter()
                .fold((0_i64, 0_i64), |(capacity, tracked), budget| {
                    let cap = budget.capacity().max(0);
                    (
                        capacity.saturating_add(cap),
                        tracked.saturating_add(budget.heap_inuse().max(0).min(cap)),
                    )
                });
        let tracked = root_tracked.saturating_add(await_free_tracked.min(await_free_cap));
        let runtime_used = stats.heap_alloc.saturating_add(stats.mem_off_heap);
        let mut capacity = self.SoftLimit();
        let magnif = self.mem_magnif.load(Ordering::Acquire);
        if self.SoftLimitConfig().2 == SoftLimitModeAuto && magnif > 0 {
            capacity = capacity.min(crate::calcRatio(self.Limit(), magnif));
        }
        let avoid = 0
            .max(runtime_used.saturating_sub(tracked))
            .max(self.Limit().saturating_sub(capacity));
        self.out_of_control.store(avoid, Ordering::Release);
        let mut deficit = self
            .Allocated()
            .saturating_sub(self.Limit())
            .saturating_add(avoid);
        if deficit > 0 {
            let mut released = 0;
            for budget in &self.await_free {
                let removed = budget.release_capacity(deficit);
                released += removed;
                deficit -= removed;
                if deficit <= 0 {
                    break;
                }
            }
            self.release(released);
        }
    }

    pub fn PoolAllocProfile(&self) -> PoolAllocProfile {
        let limit = self.Limit();
        PoolAllocProfile {
            SmallPoolLimit: (limit / 1_000).max(1),
            PoolAllocUnit: (limit / 500).max(1),
            MaxPoolAllocUnit: (limit / 100).max(1),
        }
    }

    pub fn EmplaceRootPool(&self, uid: u64) -> Result<RootPoolHandle, String> {
        self.EmplaceRootPoolWithStatus(uid)
            .map(|(_, handle)| handle)
    }

    /// 返回是否新建 root pool，以及可重复使用的 handle。
    pub fn EmplaceRootPoolWithStatus(&self, uid: u64) -> Result<(bool, RootPoolHandle), String> {
        let mut roots = self.roots.lock().expect("root pool lock poisoned");
        let created = if let std::collections::hash_map::Entry::Vacant(entry) = roots.entry(uid) {
            entry.insert(RootPoolEntry {
                quota: 0,
                max_quota: 0,
                context: None,
                running: false,
            });
            true
        } else {
            false
        };
        Ok((created, RootPoolHandle { uid }))
    }

    pub fn AddRootPool(&self, uid: u64) {
        let _ = self.EmplaceRootPool(uid);
    }

    pub fn FindRootPool(&self, uid: u64) -> Option<RootPoolHandle> {
        self.roots
            .lock()
            .expect("root pool lock poisoned")
            .contains_key(&uid)
            .then_some(RootPoolHandle { uid })
    }

    pub fn RestartEntryByContext(
        &self,
        handle: RootPoolHandle,
        context: impl Into<Option<Arc<ArbitrationContext>>>,
    ) -> bool {
        let mut roots = self.roots.lock().expect("root pool lock poisoned");
        let Some(entry) = roots.get_mut(&handle.uid) else {
            return false;
        };
        if entry.running {
            return false;
        }
        entry.context = context.into();
        entry.running = true;
        drop(roots);
        self.under_kill
            .lock()
            .expect("under-kill lock poisoned")
            .remove(&handle.uid);
        self.under_cancel
            .lock()
            .expect("under-cancel lock poisoned")
            .remove(&handle.uid);
        self.context_cache
            .lock()
            .expect("context cache lock poisoned")
            .insert(handle.uid, 0);
        true
    }

    pub fn RequestQuota(&self, handle: RootPoolHandle, request: i64) -> ArbitrateResult {
        if request <= 0 {
            self.release_root_quota(handle.uid, -request);
            return ArbitrateOk;
        }

        let context = {
            let roots = self.roots.lock().expect("root pool lock poisoned");
            let Some(entry) = roots.get(&handle.uid) else {
                return ArbitrateFail;
            };
            let Some(context) = entry.context.clone() else {
                return ArbitrateFail;
            };
            if !entry.running || !context.available() {
                return ArbitrateFail;
            }
            context
        };
        let result = Arc::new((Mutex::new(None), Condvar::new()));
        {
            self.pending
                .lock()
                .expect("pending queue lock poisoned")
                .push_back(PendingRequest {
                    handle,
                    request,
                    context: context.clone(),
                    result: result.clone(),
                    reclaim_started: None,
                });
            self.waiting_alloc.fetch_add(request, Ordering::AcqRel);
            let mut tasks = self.tasks.lock().expect("task counter lock poisoned");
            if let Some(value) = tasks.get_mut(context.memPriority.0 as usize) {
                *value += 1;
            }
            if context.waitAverse {
                tasks[ArbitrationWaitAverse] += 1;
            }
        }
        self.RunOneRound();

        let (lock, ready) = &*result;
        let mut outcome = lock.lock().expect("request result lock poisoned");
        while outcome.is_none() {
            if !context.available() {
                return ArbitrateFail;
            }
            let waited = ready
                .wait_timeout(outcome, Duration::from_millis(10))
                .expect("request result lock poisoned while waiting");
            outcome = waited.0;
            if outcome.is_none() {
                drop(outcome);
                self.RunOneRound();
                outcome = lock.lock().expect("request result lock poisoned");
            }
        }
        outcome.unwrap_or(ArbitrateFail)
    }

    /// 执行当前队列的一轮；priority 模式优先处理高优先级任务。
    pub fn RunOneRound(&self) -> i32 {
        let _round = self
            .round_mutex
            .lock()
            .expect("arbitration round lock poisoned");
        let stats = self.RuntimeStats();
        let (blocked_quota, blocked_sec) = *self
            .last_blocked
            .lock()
            .expect("blocked state lock poisoned");
        // The Rust process sampler has no Go-style last-GC heap snapshot; use
        // the current sample in its 30-second slot when that timestamp is absent.
        if self.update_mem_magnification(
            self.approxUnixTimeSec(),
            if stats.last_gc_unix_nano > 0 {
                stats.last_gc_unix_nano / 1_000_000_000
            } else {
                self.approxUnixTimeSec()
            },
            stats.heap_alloc,
            blocked_sec,
            blocked_quota,
        ) {
            self.update_tracked_heap_avoidance(stats);
        }
        self.cleanup_context_cache();
        self.refresh_pool_medium_quota();
        self.refresh_buffer_from_contexts();
        let mut executed = 0;
        loop {
            let pending = {
                let mut queue = self.pending.lock().expect("pending queue lock poisoned");
                if self.WorkMode() == ArbitratorModePriority {
                    let best = queue
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, pending)| pending.context.memPriority.0)
                        .map(|(index, _)| index);
                    best.and_then(|index| queue.remove(index))
                } else {
                    queue.pop_front()
                }
            };
            let Some(pending) = pending else {
                break;
            };
            if self.execute_pending(pending) {
                executed += 1;
            } else {
                break;
            }
        }
        executed
    }

    fn execute_pending(&self, mut pending: PendingRequest) -> bool {
        let priority = pending.context.memPriority;
        let wait_averse = pending.context.waitAverse;
        let work_mode = self.WorkMode();
        let allocated = if !pending.context.available() {
            false
        } else if work_mode == ArbitratorModeDisable {
            self.allocated.fetch_add(pending.request, Ordering::AcqRel);
            true
        } else {
            self.allocate(pending.request)
        };
        if !allocated && pending.context.available() {
            *self
                .last_blocked
                .lock()
                .expect("blocked state lock poisoned") =
                (self.Allocated(), self.approxUnixTimeSec());
        }

        if !allocated
            && pending.context.available()
            && work_mode == ArbitratorModePriority
            && !wait_averse
        {
            if pending.reclaim_started.is_none() {
                let needed = pending.request.saturating_sub(self.available());
                if self.cancel_lower_priority(priority, needed) > 0 {
                    pending.reclaim_started = Some(Instant::now());
                }
            }
            if pending
                .reclaim_started
                .is_some_and(|started| started.elapsed() < Duration::from_secs(20))
            {
                self.pending
                    .lock()
                    .expect("pending queue lock poisoned")
                    .push_front(pending);
                return false;
            }
        }

        if allocated {
            let mut roots = self.roots.lock().expect("root pool lock poisoned");
            if let Some(entry) = roots.get_mut(&pending.handle.uid) {
                entry.quota += pending.request;
                entry.max_quota = entry.max_quota.max(entry.quota);
            }
            let mut metrics = self
                .exec_metrics
                .lock()
                .expect("execution metrics lock poisoned");
            metrics.Task.Succ += 1;
            if work_mode == ArbitratorModePriority
                && let Some(value) = metrics.Task.SuccByPriority.get_mut(priority.0 as usize)
            {
                *value += 1;
            }
        } else if pending.context.available() {
            let reason = if wait_averse {
                ArbitratorWaitAverseCancel
            } else if work_mode == ArbitratorModeStandard {
                ArbitratorStandardCancel
            } else {
                ArbitratorPriorityCancel
            };
            let mut metrics = self
                .exec_metrics
                .lock()
                .expect("execution metrics lock poisoned");
            metrics.Task.Fail += 1;
            if wait_averse {
                metrics.Cancel.WaitAverse += 1;
            } else if work_mode == ArbitratorModeStandard {
                metrics.Cancel.StandardMode += 1;
            } else if let Some(value) = metrics.Cancel.PriorityMode.get_mut(priority.0 as usize) {
                *value += 1;
            }
            drop(metrics);
            pending.context.stop(reason);
        } else {
            self.exec_metrics
                .lock()
                .expect("execution metrics lock poisoned")
                .Task
                .Fail += 1;
        }

        self.waiting_alloc
            .fetch_sub(pending.request, Ordering::AcqRel);
        {
            let mut tasks = self.tasks.lock().expect("task counter lock poisoned");
            if let Some(value) = tasks.get_mut(priority.0 as usize) {
                *value -= 1;
            }
            if wait_averse {
                tasks[ArbitrationWaitAverse] -= 1;
            }
        }
        let (lock, ready) = &*pending.result;
        *lock.lock().expect("request result lock poisoned") = Some(if allocated {
            ArbitrateOk
        } else {
            ArbitrateFail
        });
        ready.notify_all();
        true
    }

    pub(crate) fn cancel_lower_priority(
        &self,
        target_priority: ArbitrationPriority,
        required: i64,
    ) -> i64 {
        if required <= 0 {
            return 0;
        }
        let now = Instant::now();
        let mut reclaimed = self
            .under_cancel
            .lock()
            .expect("under-cancel lock poisoned")
            .values()
            .filter(|(_, started)| now.duration_since(*started) < Duration::from_secs(20))
            .map(|(quota, _)| *quota)
            .sum::<i64>();
        if reclaimed >= required {
            return reclaimed;
        }
        let mut candidates: Vec<_> = self
            .roots
            .lock()
            .expect("root pool lock poisoned")
            .iter()
            .filter(|(_, entry)| entry.running && entry.quota > 0)
            .filter_map(|(uid, entry)| {
                entry
                    .context
                    .clone()
                    .map(|context| (*uid, entry.quota, context))
            })
            .filter(|(_, _, context)| context.memPriority.0 < target_priority.0)
            .collect();
        candidates.sort_by_key(|(_, quota, context)| (context.memPriority.0, -*quota));
        for (uid, quota, context) in candidates {
            if !context.available() {
                continue;
            }
            if let Some(value) = self
                .exec_metrics
                .lock()
                .expect("execution metrics lock poisoned")
                .Cancel
                .PriorityMode
                .get_mut(context.memPriority.0 as usize)
            {
                *value += 1;
            }
            context.stop(ArbitratorPriorityCancel);
            self.under_cancel
                .lock()
                .expect("under-cancel lock poisoned")
                .insert(uid, (quota, now));
            reclaimed += quota;
            if reclaimed >= required {
                break;
            }
        }
        reclaimed
    }

    fn cancel_for_oom(&self) {
        let now = Instant::now();
        let mut reclaimed = self
            .under_kill
            .lock()
            .expect("under-kill lock poisoned")
            .values()
            .filter(|(_, started)| now.duration_since(*started) < Duration::from_secs(20))
            .map(|(heap, _)| *heap)
            .sum::<i64>();
        let cached_ids: Vec<_> = self
            .context_cache
            .lock()
            .expect("context cache lock poisoned")
            .keys()
            .copied()
            .collect();
        let entries: Vec<_> = {
            let roots = self.roots.lock().expect("root pool lock poisoned");
            cached_ids
                .iter()
                .filter_map(|uid| roots.get(uid).map(|entry| (*uid, entry)))
                .filter(|(_, entry)| entry.running)
                .filter_map(|(uid, entry)| {
                    entry
                        .context
                        .clone()
                        .map(|context| (uid, entry.quota, context))
                })
                .collect()
        };
        let mut candidates: Vec<_> = entries
            .into_iter()
            .filter_map(|(uid, quota, context)| {
                let heap_inuse = context
                    .helper
                    .read()
                    .expect("arbitration helper lock poisoned")
                    .as_ref()
                    .map_or(0, |helper| helper.MemUsage().HeapInuse);
                (heap_inuse > 0).then_some((context.memPriority.0, quota, heap_inuse, uid, context))
            })
            .collect();
        // Go scans higher quota shards first within each priority, then the
        // context cache for entries that hold heap without a root quota.
        candidates
            .sort_by_key(|(priority, quota, heap_inuse, _, _)| (*priority, -*quota, -*heap_inuse));
        let required = (self.heap_inuse.load(Ordering::Acquire) - self.memRisk()).max(0);
        if reclaimed >= required {
            return;
        }
        for (_, _, heap_inuse, uid, context) in candidates {
            if context.available() {
                if let Some(value) = self
                    .exec_metrics
                    .lock()
                    .expect("execution metrics lock poisoned")
                    .Risk
                    .OOMKill
                    .get_mut(context.memPriority.0 as usize)
                {
                    *value += 1;
                }
                context.stop(ArbitratorOOMRiskKill);
                self.under_kill
                    .lock()
                    .expect("under-kill lock poisoned")
                    .insert(uid, (heap_inuse, now));
                reclaimed += heap_inuse;
                if reclaimed >= required {
                    break;
                }
            }
        }
    }

    fn release_root_quota(&self, uid: u64, amount: i64) {
        let released = {
            let mut roots = self.roots.lock().expect("root pool lock poisoned");
            let Some(entry) = roots.get_mut(&uid) else {
                return;
            };
            let released = amount.max(0).min(entry.quota);
            entry.quota -= released;
            released
        };
        self.release(released);
    }

    pub fn ResetRootPoolByID(&self, uid: u64, max_mem_consumed: i64, tune: bool) -> bool {
        let released = {
            let mut roots = self.roots.lock().expect("root pool lock poisoned");
            let Some(entry) = roots.get_mut(&uid) else {
                return false;
            };
            if !entry.running {
                return false;
            }
            let released = entry.quota;
            entry.quota = 0;
            entry.max_quota = entry.max_quota.max(max_mem_consumed);
            entry.running = false;
            entry.context = None;
            released
        };
        self.release(released);
        self.under_kill
            .lock()
            .expect("under-kill lock poisoned")
            .remove(&uid);
        self.under_cancel
            .lock()
            .expect("under-cancel lock poisoned")
            .remove(&uid);
        self.context_cache
            .lock()
            .expect("context cache lock poisoned")
            .insert(uid, self.approxUnixTimeSec());
        if tune && max_mem_consumed > self.PoolAllocProfile().SmallPoolLimit {
            self.record_mem_consumed(max_mem_consumed);
        }
        true
    }

    pub fn RemoveRootPoolByID(&self, uid: u64) -> bool {
        let entry = self
            .roots
            .lock()
            .expect("root pool lock poisoned")
            .remove(&uid);
        if let Some(entry) = entry {
            self.under_kill
                .lock()
                .expect("under-kill lock poisoned")
                .remove(&uid);
            self.under_cancel
                .lock()
                .expect("under-cancel lock poisoned")
                .remove(&uid);
            self.context_cache
                .lock()
                .expect("context cache lock poisoned")
                .remove(&uid);
            self.release(entry.quota);
            true
        } else {
            false
        }
    }

    pub fn RootPoolNum(&self) -> i64 {
        self.roots.lock().expect("root pool lock poisoned").len() as i64
    }

    pub fn RootPoolCount(&self) -> usize {
        self.RootPoolNum() as usize
    }

    pub fn TaskNum(&self) -> i64 {
        self.TaskNumByPattern()[..3].iter().sum()
    }

    pub fn WaitingAllocSize(&self) -> i64 {
        self.waiting_alloc.load(Ordering::Acquire)
    }

    pub fn SetDigestProfileCacheLimit(&self, limit: i64) {
        self.digest_profile_limit
            .store(limit.clamp(0, 9_000_000_000_000_000), Ordering::Release);
        self.shrink_digest_profiles();
    }

    pub fn UpdateDigestProfileCache(&self, digest_id: u64, mem_consumed: i64, utime_sec: i64) {
        if digest_id == 0 {
            return;
        }
        let mut profiles = self
            .digest_profiles
            .lock()
            .expect("digest profile lock poisoned");
        let profile = profiles.entry(digest_id).or_insert(DigestProfile {
            max_value: 0,
            last_fetch_unix_sec: utime_sec,
            timed_max: [(i64::MIN, 0); DIGEST_PROFILE_WINDOW_SLOTS],
        });
        let time_slot = utime_sec / DIGEST_PROFILE_TIME_ALIGN_SEC;
        let slot_index = time_slot.rem_euclid(DIGEST_PROFILE_WINDOW_SLOTS as i64) as usize;
        let slot = &mut profile.timed_max[slot_index];
        if slot.0 != time_slot {
            *slot = (time_slot, 0);
        }
        if mem_consumed > slot.1 {
            slot.1 = mem_consumed;
            profile.max_value = profile
                .timed_max
                .iter()
                .filter(|(slot_time, _)| {
                    *slot_time > time_slot - DIGEST_PROFILE_ACTIVE_SLOTS && *slot_time <= time_slot
                })
                .map(|(_, value)| *value)
                .max()
                .unwrap_or(0);
        }
        profile.last_fetch_unix_sec = profile.last_fetch_unix_sec.max(utime_sec);
        drop(profiles);
        self.shrink_digest_profiles();
    }

    pub fn GetDigestProfileCache(&self, digest_id: u64, utime_sec: i64) -> Option<i64> {
        if digest_id == 0 {
            return None;
        }
        let mut profiles = self
            .digest_profiles
            .lock()
            .expect("digest profile lock poisoned");
        let profile = profiles.get_mut(&digest_id)?;
        profile.last_fetch_unix_sec = profile.last_fetch_unix_sec.max(utime_sec);
        Some(profile.max_value)
    }

    fn shrink_digest_profiles(&self) {
        let limit = self.digest_profile_limit.load(Ordering::Acquire).max(0) as usize;
        let mut profiles = self
            .digest_profiles
            .lock()
            .expect("digest profile lock poisoned");
        if profiles.len() <= limit {
            return;
        }
        self.exec_metrics
            .lock()
            .expect("execution metrics lock poisoned")
            .ShrinkDigest += 1;
        let retain = limit / 2;
        let mut by_value: Vec<_> = profiles
            .iter()
            .map(|(digest, profile)| (*digest, profile.max_value))
            .collect();
        by_value.sort_by_key(|(_, value)| *value);
        for (digest, _) in by_value.into_iter().take(profiles.len() - retain) {
            profiles.remove(&digest);
        }
    }

    fn await_free_index(&self, uid: u64) -> usize {
        let mut value = uid;
        value = (!value).wrapping_add(value << 21);
        value ^= value >> 24;
        value = value.wrapping_add(value << 3).wrapping_add(value << 8);
        value ^= value >> 14;
        value = value.wrapping_add(value << 2).wrapping_add(value << 4);
        value ^= value >> 28;
        value = value.wrapping_add(value << 31);
        value as usize & (self.await_free.len() - 1)
    }

    pub fn GetAwaitFreeBudgets(&self, uid: u64) -> Arc<TrackedConcurrentBudget> {
        self.await_free[self.await_free_index(uid)].clone()
    }

    pub fn ConsumeQuotaFromAwaitFreePool(&self, uid: u64, req: i64) -> bool {
        let budget = self.GetAwaitFreeBudgets(uid);
        if budget.ConsumeQuota(self.approxUnixTimeSec(), req).is_ok() {
            self.exec_metrics
                .lock()
                .expect("execution metrics lock poisoned")
                .AwaitFree
                .Succ += 1;
            return true;
        }
        if req <= 0 {
            self.exec_metrics
                .lock()
                .expect("execution metrics lock poisoned")
                .AwaitFree
                .Succ += 1;
            return true;
        }
        let deficit = (budget.used() - budget.capacity()).max(0);
        let reserve = deficit.max(self.PoolAllocProfile().PoolAllocUnit);
        let stats = self.RuntimeStats();
        if self.AtOOMRisk()
            || stats.heap_alloc.saturating_add(stats.mem_off_heap) > self.oomRisk() - reserve
            || !self.allocate(reserve)
        {
            self.exec_metrics
                .lock()
                .expect("execution metrics lock poisoned")
                .AwaitFree
                .Fail += 1;
            return false;
        }
        budget.Reserve(budget.capacity().saturating_add(reserve));
        self.exec_metrics
            .lock()
            .expect("execution metrics lock poisoned")
            .AwaitFree
            .Succ += 1;
        true
    }

    pub fn ReportHeapInuseToAwaitFreePool(&self, uid: u64, req: i64) {
        self.GetAwaitFreeBudgets(uid).ReportHeapInuse(req);
    }

    /// 回收空闲 await-free 分片容量，并将配额归还仲裁器。
    pub fn ShrinkAwaitFreePool(&self, min_remain: i64) -> i64 {
        self.exec_metrics
            .lock()
            .expect("execution metrics lock poisoned")
            .AwaitFree
            .Shrink += 1;
        let mut released = 0;
        for budget in &self.await_free {
            if budget.used() <= 0 && budget.capacity() > min_remain {
                let capacity = budget.budget.Stop();
                let keep = min_remain.max(0).min(capacity);
                if keep > 0 {
                    budget.Reserve(keep);
                }
                released += capacity - keep;
            }
        }
        self.release(released);
        released
    }

    pub fn SuggestPoolInitCap(&self) -> i64 {
        self.pool_medium_quota.load(Ordering::Acquire)
    }

    /// 启动轻量异步运行循环；重复启动返回 false。
    pub fn StartAutoRun(self: &Arc<Self>, tick: Duration) -> bool {
        if self.running.swap(true, Ordering::AcqRel) {
            return false;
        }
        let weak = Arc::downgrade(self);
        let interval = tick.max(Duration::from_millis(1));
        let worker = thread::spawn(move || {
            while let Some(arbitrator) = weak.upgrade() {
                if !arbitrator.running.load(Ordering::Acquire) {
                    break;
                }
                arbitrator.setUnixTimeSec(now_unix_sec());
                arbitrator.RunOneRound();
                thread::sleep(interval);
            }
        });
        *self.worker.lock().expect("worker lock poisoned") = Some(worker);
        true
    }

    /// 停止异步循环并等待线程退出。
    pub fn StopAutoRun(&self) -> bool {
        if !self.running.swap(false, Ordering::AcqRel) {
            return false;
        }
        if let Some(worker) = self.worker.lock().expect("worker lock poisoned").take() {
            let _ = worker.join();
        }
        self.RunOneRound();
        true
    }

    pub fn IsRunning(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

fn now_unix_sec() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// 堆释放速度过慢或超过回收检查窗口时判定“挂起风险”。
pub fn memHangRisk(
    free_speed_bps: i64,
    min_heap_free_speed_bps: i64,
    now: SystemTime,
    start_time: SystemTime,
) -> bool {
    free_speed_bps < min_heap_free_speed_bps
        || now.duration_since(start_time).unwrap_or_default() > defHeapReclaimCheckMaxDuration
}
