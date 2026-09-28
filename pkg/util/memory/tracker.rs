// Copyright 2018 PingCAP, Inc.
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

// 查询执行期内存 Tracker 树与配额动作。
//
// 对应 Go `tracker.go`：父子消费累加、软/硬配额与超限 Action、缓冲记账、
// 全局 Tracker，以及可选 `mem-arbitrator` 下的 small/big budget 切换。
// 下方已有逐段中文说明；本模块文档汇总导航职责。

// TiDB 查询执行期间的内存 Tracker 树、配额检查、超限动作触发、GC 感知释放记录，
// 以及全局内存仲裁器从 small budget 切换到 big budget 的主要控制流。

#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(unused_variables)]

use std::collections::HashMap;
use std::ptr;
#[cfg(feature = "mem-arbitrator")]
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(feature = "mem-arbitrator")]
use std::sync::atomic::AtomicI32;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

#[cfg(feature = "mem-arbitrator")]
use crate::HashStr;
#[cfg(feature = "mem-arbitrator")]
use crate::arbitrator::{
    ArbitrateHelper, ArbitrateOk, ArbitrationContext, ArbitrationPriority,
    ArbitrationPriorityMedium, ArbitratorStopReason, CancelReceiver, ConcurrentBudget,
    MemArbitrator, RootPoolHandle, TrackedConcurrentBudget,
};
#[cfg(feature = "mem-arbitrator")]
use crate::global_arbitrator::GlobalMemArbitrator;
use crate::sqlkiller;

const byteSize: i64 = 1;
const byteSizeKB: i64 = 1024;
const byteSizeMB: i64 = 1024 * byteSizeKB;
const byteSizeGB: i64 = 1024 * byteSizeMB;

/// 近似 Go `atomicutil` 的原子包装，供全局配额开关与计数使用。
pub mod atomicutil {
    use super::*;

    pub struct Int64(AtomicI64);
    impl Int64 {
        pub const fn new(value: i64) -> Self {
            Self(AtomicI64::new(value))
        }
        pub fn Load(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
        pub fn Store(&self, value: i64) {
            self.0.store(value, Ordering::SeqCst)
        }
        pub fn Add(&self, value: i64) -> i64 {
            self.0.fetch_add(value, Ordering::SeqCst) + value
        }
        pub fn Swap(&self, value: i64) -> i64 {
            self.0.swap(value, Ordering::SeqCst)
        }
        pub fn CompareAndSwap(&self, old: i64, new: i64) -> bool {
            self.0
                .compare_exchange(old, new, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        }
    }

    pub struct Uint64(AtomicU64);
    impl Uint64 {
        pub const fn new(value: u64) -> Self {
            Self(AtomicU64::new(value))
        }
        pub fn Load(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
        pub fn Store(&self, value: u64) {
            self.0.store(value, Ordering::SeqCst)
        }
    }

    pub struct Bool(AtomicBool);
    impl Bool {
        pub const fn new(value: bool) -> Self {
            Self(AtomicBool::new(value))
        }
        pub fn Load(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        pub fn Store(&self, value: bool) {
            self.0.store(value, Ordering::SeqCst)
        }
    }

    pub struct String(&'static str);
    impl String {
        pub const fn new(value: &'static str) -> Self {
            Self(value)
        }
        pub fn Load(&self) -> &'static str {
            self.0
        }
    }

    pub struct Time(Mutex<SystemTime>);
    impl Time {
        pub const fn new(value: SystemTime) -> Self {
            Self(Mutex::new(value))
        }
        pub fn Load(&self) -> SystemTime {
            *self.0.lock().unwrap()
        }
        pub fn Store(&self, value: SystemTime) {
            *self.0.lock().unwrap() = value
        }
    }
}

/// 内存超限动作接口：可挂 fallback 链并按优先级排序。
pub trait ActionOnExceed: Send {
    fn Action(&mut self, tracker: &mut Tracker);
    fn SetFallback(&mut self, action: Option<Box<dyn ActionOnExceed>>);
    fn GetFallback(&mut self) -> Option<Box<dyn ActionOnExceed>>;
    fn GetPriority(&self) -> i64;
    fn SetFinished(&mut self);
    fn IsFinished(&self) -> bool;
}

/// 默认硬限动作：首次超限仅标记，后续走 fallback。
#[derive(Default)]
pub struct LogOnExceed {
    fallback: Option<Box<dyn ActionOnExceed>>,
    acted: bool,
    finished: bool,
}

impl ActionOnExceed for LogOnExceed {
    fn Action(&mut self, tracker: &mut Tracker) {
        if self.acted {
            if let Some(fallback) = self.fallback.as_mut() {
                fallback.Action(tracker);
            }
        } else {
            self.acted = true;
        }
    }
    fn SetFallback(&mut self, action: Option<Box<dyn ActionOnExceed>>) {
        self.fallback = action;
    }
    fn GetFallback(&mut self) -> Option<Box<dyn ActionOnExceed>> {
        self.fallback.take()
    }
    fn GetPriority(&self) -> i64 {
        1
    }
    fn SetFinished(&mut self) {
        self.finished = true;
    }
    fn IsFinished(&self) -> bool {
        self.finished
    }
}

mod intest {
    pub fn Assert(ok: bool, message: String) {
        assert!(ok, "{}", message);
    }
}

mod metrics {
    pub struct Gauge;
    impl Gauge {
        pub fn WithLabelValues(&self, _a: &str, _b: &str) -> &Self {
            self
        }
        pub fn Set(&self, _v: f64) {}
    }
    pub static MemoryUsage: Gauge = Gauge;
}

fn UsingGlobalMemArbitration() -> bool {
    crate::global_arbitrator::UsingGlobalMemArbitration()
}

// TrackMemWhenExceeds is the threshold when memory usage needs to be tracked.
// TrackMemWhenExceeds 对应 Go 中延迟记账的阈值，累计超过 100MB 后才真正 Consume/Release。
pub const TrackMemWhenExceeds: i64 = 104_857_600; // 100MB

// DefMemQuotaQuery is default memory quota for query.
// DefMemQuotaQuery 是单条查询默认硬配额，软配额会在此基础上按 softScale 推导。
pub const DefMemQuotaQuery: i64 = 1_073_741_824; // 1GB

// Process global variables for memory limit.
/// 进程级内存限制相关全局变量（原始文本 / 字节上限 / session 最小尺寸）。
pub static ServerMemoryLimitOriginText: atomicutil::String = atomicutil::String::new("0");
pub static ServerMemoryLimit: atomicutil::Uint64 = atomicutil::Uint64::new(0);
pub static ServerMemoryLimitSessMinSize: atomicutil::Uint64 = atomicutil::Uint64::new(128 << 20);

/// 强制落盘、触发内存 GC 及 GC 统计相关全局标志。
pub static QueryForceDisk: atomicutil::Int64 = atomicutil::Int64::new(0);
pub static TriggerMemoryLimitGC: atomicutil::Bool = atomicutil::Bool::new(false);
pub static MemoryLimitGCLast: atomicutil::Time = atomicutil::Time::new(SystemTime::UNIX_EPOCH);
pub static MemoryLimitGCTotal: atomicutil::Int64 = atomicutil::Int64::new(0);

// Tracker is used to track the memory usage during query execution.
// It contains an optional limit and can be arranged into a tree structure
// such that the consumption tracked by a Tracker is also tracked by
// its ancestors. The main idea comes from Apache Impala:
// https://github.com/cloudera/Impala/blob/cdh5-trunk/be/src/runtime/mem-tracker.h
// By default, memory consumption is tracked via calls to "Consume()", either to
// the tracker itself or to one of its descendents. A typical sequence of calls
// for a single Tracker is:
// 1. tracker.SetLabel() / tracker.SetActionOnExceed() / tracker.AttachTo()
// 2. tracker.Consume() / tracker.ReplaceChild() / tracker.BytesConsumed()
// NOTE: We only protect concurrent access to "bytesConsumed" and "children",
// that is to say:
// 1. Only "BytesConsumed()", "Consume()" and "AttachTo()" are thread-safe.
// 2. Other operations of a Tracker tree is not thread-safe.
// We have two limits for the memory quota: soft limit and hard limit.
// If the soft limit is exceeded, we will trigger the action that alleviates the
// speed of memory growth. The soft limit is hard-coded as `0.8*hard limit`.
// The actions that could be triggered are: AggSpillDiskAction.
// If the hard limit is exceeded, we will trigger the action that immediately
// reduces memory usage. The hard limit is set by the system variable `tidb_mem_query_quota`.
// The actions that could be triggered are: SpillDiskAction, SortAndSpillDiskAction, rateLimitAction,
// PanicOnExceed, globalPanicOnExceed, LogOnExceed.
// Tracker 保留 Go 里的父子树结构：子节点 Consume 时会一路累加到祖先节点。
// parent 使用 AtomicPtr；bytesLimit 用 Mutex 原子发布 hard/soft 配置对。
/// 查询执行内存追踪器：树形累加消费并触发软/硬限动作。
pub struct Tracker {
    parent: AtomicPtr<Tracker>,
    #[cfg(feature = "mem-arbitrator")]
    pub MemArbitrator: Option<Box<memArbitrator>>,
    pub Killer: Option<Box<sqlkiller::SQLKiller>>,
    bytesLimit: Mutex<bytesLimits>,
    actionMuForHardLimit: actionMu,
    actionMuForSoftLimit: actionMu,
    mu: TrackerChildrenMu,
    label: i32,
    // following fields are used with atomic operations, so make them 64-byte aligned.
    // Go 通过字段顺序强调 64 字节对齐；只保留原子访问意图，未处理真实 cache-line 布局。
    bytesReleased: AtomicI64,
    maxConsumed: atomicutil::Int64,
    pub SessionID: atomicutil::Uint64,
    bytesConsumed: AtomicI64,
    pub IsRootTrackerOfSess: bool,
    isGlobal: bool,
}

// Children and the parent link are only mutated by the documented tree APIs;
// concurrent accounting touches atomics, immutable limits snapshots, and locked state.
unsafe impl Send for Tracker {}
unsafe impl Sync for Tracker {}

// Go 的匿名 mu struct 拆成命名结构，便于表达 children map 与互斥锁的绑定关系。
struct TrackerChildrenMu {
    children: Mutex<Option<HashMap<i32, Vec<*mut Tracker>>>>,
}

impl Default for TrackerChildrenMu {
    fn default() -> Self {
        Self {
            children: Mutex::new(None),
        }
    }
}

// actionMu 保护一个超限动作链。Go 代码里 hard/soft limit 各有一个锁和 ActionOnExceed。
struct actionMu {
    actionOnExceed: Mutex<Option<Box<dyn ActionOnExceed>>>,
}

impl Default for actionMu {
    fn default() -> Self {
        Self {
            actionOnExceed: Mutex::new(None),
        }
    }
}

// EnableGCAwareMemoryTrack is used to turn on/off the GC-aware memory track
// EnableGCAwareMemoryTrack 控制 Release 是否等待 Go GC finalizer 回调后再扣减 released 统计。
pub static EnableGCAwareMemoryTrack: atomicutil::Bool = atomicutil::Bool::new(false);

// https://golang.google.cn/pkg/runtime/#SetFinalizer
// It is not guaranteed that a finalizer will run if the size of *obj is zero bytes.
// finalizerRef 在 Go 中用一个字节避免零大小对象 finalizer 不运行；只保留这个迁移原因。
struct finalizerRef {
    _unused: u8,
}

// softScale means the scale of the soft limit to the hard limit.
// softScale 表示软配额占硬配额的比例，Go 使用 float64 计算后转 int64。
const softScale: f64 = 0.8;

// bytesLimits holds limit config atomically.
// bytesLimits 作为整体被 AtomicPtr 发布，避免 hard/soft limit 被不同线程看到不一致的组合。
#[derive(Clone, Copy)]
struct bytesLimits {
    bytesHardLimit: i64, // bytesHardLimit <= 0 means no limit, used for actionMuForHardLimit.
    bytesSoftLimit: i64, // bytesSoftLimit <= 0 means no limit, used for actionMuForSoftLimit.
}

fn limits_for(bytes_limit: i64) -> bytesLimits {
    let hard = if bytes_limit <= 0 { -1 } else { bytes_limit };
    bytesLimits {
        bytesHardLimit: hard,
        bytesSoftLimit: if hard <= 0 {
            -1
        } else {
            ((hard as f64) * softScale) as i64
        },
    }
}

// MemUsageTop1Tracker record the use memory top1 session's tracker for kill.
// MemUsageTop1Tracker 记录当前内存使用最高的 session root tracker，用于全局内存限制下的 kill 策略。
pub static MemUsageTop1Tracker: AtomicPtr<Tracker> = AtomicPtr::new(ptr::null_mut());

// mockDebugInject 是测试注入点。Go 里只在 intest.InTest 分支调用，保持 unsafe 全局占位。
static mut mockDebugInject: Option<fn()> = None;

impl Tracker {
    // new_empty 是 Rust 里的构造辅助，用来表达 Go 中 &Tracker{label: label} 后再填字段的模式。
    fn new_empty(label: i32) -> Self {
        Self {
            parent: AtomicPtr::new(ptr::null_mut()),
            #[cfg(feature = "mem-arbitrator")]
            MemArbitrator: None,
            Killer: None,
            bytesLimit: Mutex::new(limits_for(-1)),
            actionMuForHardLimit: actionMu::default(),
            actionMuForSoftLimit: actionMu::default(),
            mu: TrackerChildrenMu::default(),
            label,
            bytesReleased: AtomicI64::new(0),
            maxConsumed: atomicutil::Int64::new(0),
            SessionID: atomicutil::Uint64::new(0),
            bytesConsumed: AtomicI64::new(0),
            IsRootTrackerOfSess: false,
            isGlobal: false,
        }
    }
}

// InitTracker initializes a memory tracker.
//  1. "label" is the label used in the usage string.
//  2. "bytesLimit <= 0" means no limit.
// For the common tracker, isGlobal is default as false
// InitTracker 对应 Go 的原地初始化函数，调用者传入已有 Tracker，函数负责清空父子关系和动作。
pub fn InitTracker(
    t: &mut Tracker,
    label: i32,
    bytesLimit: i64,
    action: Option<Box<dyn ActionOnExceed>>,
) {
    *t.mu.children.lock().unwrap() = None;
    *t.actionMuForHardLimit.actionOnExceed.lock().unwrap() = action;
    *t.actionMuForSoftLimit.actionOnExceed.lock().unwrap() = None;
    t.parent.store(ptr::null_mut(), Ordering::SeqCst);

    t.label = label;
    *t.bytesLimit.lock().unwrap() = limits_for(bytesLimit);
    t.maxConsumed.Store(0);
    t.isGlobal = false;
}

// NewTracker creates a memory tracker.
//  1. "label" is the label used in the usage string.
//  2. "bytesLimit <= 0" means no limit.
// For the common tracker, isGlobal is default as false
// NewTracker 创建普通 tracker，默认 hard-limit 动作为 LogOnExceed。
pub fn NewTracker(label: i32, bytesLimit: i64) -> Box<Tracker> {
    let mut t = Box::new(Tracker::new_empty(label));
    *t.bytesLimit.lock().unwrap() = limits_for(bytesLimit);
    *t.actionMuForHardLimit.actionOnExceed.lock().unwrap() = Some(Box::new(LogOnExceed::default()));
    t.isGlobal = false;
    t
}

// NewGlobalTracker creates a global tracker, its isGlobal is default as true
// NewGlobalTracker 与 NewTracker 基本一致，但标记为全局 tracker，不维护 children 以减少锁竞争。
pub fn NewGlobalTracker(label: i32, bytesLimit: i64) -> Box<Tracker> {
    let mut t = Box::new(Tracker::new_empty(label));
    *t.bytesLimit.lock().unwrap() = limits_for(bytesLimit);
    *t.actionMuForHardLimit.actionOnExceed.lock().unwrap() = Some(Box::new(LogOnExceed::default()));
    t.isGlobal = true;
    t
}

impl Tracker {
    /// Rust constructor equivalent to `NewTracker`, retained for callers that
    /// use an owned tracker value rather than Go-style boxed construction.
    pub fn new(label: i32, bytes_limit: i64) -> Self {
        *NewTracker(label, bytes_limit)
    }

    // CheckBytesLimit check whether the bytes limit of the tracker is equal to a value.
    // Only used in test.
    // CheckBytesLimit 只服务测试，直接读取当前 hard limit 指针中的值。
    pub fn CheckBytesLimit(&self, val: i64) -> bool {
        self.bytesLimit.lock().unwrap().bytesHardLimit == val
    }

    // SetBytesLimit sets the bytes limit for this tracker.
    // "bytesHardLimit <= 0" means no limit.
    // SetBytesLimit 按 Go 语义原子替换整组 hard/soft limit；不会单独更新字段。
    pub fn SetBytesLimit(&self, bytesLimit: i64) {
        *self.bytesLimit.lock().unwrap() = limits_for(bytesLimit);
    }

    // GetBytesLimit gets the bytes limit for this tracker.
    // "bytesHardLimit <= 0" means no limit.
    // GetBytesLimit 返回 hard limit，soft limit 只在 Consume 的超限分支使用。
    pub fn GetBytesLimit(&self) -> i64 {
        self.bytesLimit.lock().unwrap().bytesHardLimit
    }

    // CheckExceed checks whether the consumed bytes is exceed for this tracker.
    // CheckExceed 只检查 hard limit，且 bytesHardLimit <= 0 表示没有限制。
    pub fn CheckExceed(&self) -> bool {
        let bytesHardLimit = self.bytesLimit.lock().unwrap().bytesHardLimit;
        self.bytesConsumed.load(Ordering::SeqCst) >= bytesHardLimit && bytesHardLimit > 0
    }

    // SetActionOnExceed sets the action when memory usage exceeds bytesHardLimit.
    // SetActionOnExceed 替换 hard limit 超限动作；锁粒度沿用 Go 的 actionMu。
    pub fn SetActionOnExceed(&self, a: Option<Box<dyn ActionOnExceed>>) {
        *self.actionMuForHardLimit.actionOnExceed.lock().unwrap() = a;
    }

    // FallbackOldAndSetNewAction sets the action when memory usage exceeds bytesHardLimit
    // and set the original action as its fallback.
    // FallbackOldAndSetNewAction 把新动作和旧 fallback 链按优先级重排后挂回 hard limit。
    pub fn FallbackOldAndSetNewAction(&self, a: Option<Box<dyn ActionOnExceed>>) {
        let mut guard = self.actionMuForHardLimit.actionOnExceed.lock().unwrap();
        let old = guard.take();
        *guard = reArrangeFallback(a, old);
    }

    // FallbackOldAndSetNewActionForSoftLimit sets the action when memory usage exceeds bytesSoftLimit
    // and set the original action as its fallback.
    // FallbackOldAndSetNewActionForSoftLimit 与 hard limit 版本相同，只是操作 soft limit 动作链。
    pub fn FallbackOldAndSetNewActionForSoftLimit(&self, a: Option<Box<dyn ActionOnExceed>>) {
        let mut guard = self.actionMuForSoftLimit.actionOnExceed.lock().unwrap();
        let old = guard.take();
        *guard = reArrangeFallback(a, old);
    }

    // GetFallbackForTest get the oom action used by test.
    // GetFallbackForTest 用于测试观察当前 OOM 动作；可选择跳过已经 finished 的动作。
    pub fn GetFallbackForTest(
        &self,
        ignoreFinishedAction: bool,
    ) -> Option<Box<dyn ActionOnExceed>> {
        let mut guard = self.actionMuForHardLimit.actionOnExceed.lock().unwrap();
        if let Some(action) = guard.as_mut() {
            if action.IsFinished() && ignoreFinishedAction {
                // Go 代码会把 actionOnExceed 改为 fallback；用 take/set 表达所有权迁移。
                *guard = action.GetFallback();
            }
        }
        guard.take()
    }

    // UnbindActions unbinds actionForHardLimit and actionForSoftLimit.
    // UnbindActions 在 statement reset 路径解绑 hard/soft limit 动作，后续会重新 SetActionOnExceed。
    pub fn UnbindActions(&self) {
        *self.actionMuForSoftLimit.actionOnExceed.lock().unwrap() = None;

        // 当前 Go 注释说明 ResetContextOfStmt 会随后设置 hard limit 动作，因此这里清空是安全的。
        *self.actionMuForHardLimit.actionOnExceed.lock().unwrap() = None;
    }

    // UnbindActionFromHardLimit unbinds action from hardLimit.
    // UnbindActionFromHardLimit 从 hard limit fallback 链中移除指定动作，保持其它链路顺序。
    pub fn UnbindActionFromHardLimit(&self, actionToUnbind: *const dyn ActionOnExceed) {
        let mut guard = self.actionMuForHardLimit.actionOnExceed.lock().unwrap();
        *guard = unbind_action(guard.take(), actionToUnbind);
    }
}

// Consume and rebuild the owned fallback chain so every traversed Box remains alive.
// This preserves Go's identity-based removal without retaining pointers into temporaries.
fn unbind_action(
    chain: Option<Box<dyn ActionOnExceed>>,
    action_to_unbind: *const dyn ActionOnExceed,
) -> Option<Box<dyn ActionOnExceed>> {
    let mut current = chain?;
    if std::ptr::addr_eq(&*current as *const dyn ActionOnExceed, action_to_unbind) {
        return current.GetFallback();
    }
    let fallback = unbind_action(current.GetFallback(), action_to_unbind);
    current.SetFallback(fallback);
    Some(current)
}

// reArrangeFallback merge two action chains and rearrange them by priority in descending order.
// reArrangeFallback 递归合并两个 fallback 链，优先级高的动作排在前面。
pub fn reArrangeFallback(
    a: Option<Box<dyn ActionOnExceed>>,
    b: Option<Box<dyn ActionOnExceed>>,
) -> Option<Box<dyn ActionOnExceed>> {
    if a.is_none() {
        return b;
    }
    if b.is_none() {
        return a;
    }
    let mut a = a.unwrap();
    let mut b = b.unwrap();
    // Go 中低优先级会和 b 交换，保证当前链头是更高优先级。
    if a.GetPriority() < b.GetPriority() {
        std::mem::swap(&mut a, &mut b);
    }
    let next = reArrangeFallback(a.GetFallback(), Some(b));
    a.SetFallback(next);
    Some(a)
}

impl Tracker {
    // SetLabel sets the label of a Tracker.
    // SetLabel 改 label 前先从父节点摘除，改完后再挂回，确保父节点 children map 的 key 同步更新。
    pub fn SetLabel(&mut self, label: i32) {
        let parent = self.getParent();
        self.Detach();
        self.label = label;
        if !parent.is_null() {
            self.AttachTo(parent);
        }
    }

    // Label gets the label of a Tracker.
    // Label 返回 tracker 的整数标签，和 Go 方法保持同名。
    pub fn Label(&self) -> i32 {
        self.label
    }

    // AttachTo attaches this memory tracker as a child to another Tracker. If it
    // already has a parent, this function will remove it from the old parent.
    // Its consumed memory usage is used to update all its ancestors.
    // AttachTo 把当前 tracker 挂为 parent 子节点，并把已有消费量补记到账父链。
    pub fn AttachTo(&mut self, parent: *mut Tracker) {
        unsafe {
            if (*parent).isGlobal {
                self.AttachToGlobalTracker(parent);
                return;
            }
            let oldParent = self.getParent();
            if !oldParent.is_null() {
                (*oldParent).remove(self);
            }
            {
                let mut children_guard = (*parent).mu.children.lock().unwrap();
                let children = children_guard.get_or_insert_with(HashMap::new);
                children
                    .entry(self.label)
                    .or_insert_with(Vec::new)
                    .push(self);
            }

            self.setParent(parent);
            (*parent).Consume(self.BytesConsumed());
        }
    }

    // Detach de-attach the tracker child from its parent, then set its parent property as nil
    // Detach 从父 tracker 摘除当前节点，并在必要时解绑 session root 的动作和内存仲裁器。
    pub fn Detach(&mut self) {
        let parent = self.getParent();
        if parent.is_null() {
            return;
        }
        unsafe {
            if (*parent).isGlobal {
                self.DetachFromGlobalTracker();
                return;
            }
            // exception 记录 session 是否已经被 kill；该信息会影响 DetachMemArbitrator 是否更新 digest cache。
            #[cfg(feature = "mem-arbitrator")]
            let mut exception = false;
            #[cfg(feature = "mem-arbitrator")]
            if let Some(m) = self.MemArbitrator.as_ref() {
                if let Some(killer) = m.helper.killer.as_ref() {
                    exception = killer.GetKillSignal() != sqlkiller::UnspecifiedKillSignal;
                }
            }
            if (*parent).IsRootTrackerOfSess && self.label != LabelForMemDB {
                *(*parent)
                    .actionMuForHardLimit
                    .actionOnExceed
                    .lock()
                    .unwrap() = None;
                *(*parent)
                    .actionMuForSoftLimit
                    .actionOnExceed
                    .lock()
                    .unwrap() = None;
                if let Some(killer) = (*parent).Killer.as_mut() {
                    killer.Reset();
                }
            }
            (*parent).remove(self);
            self.setParent(ptr::null_mut()); // atomic operator
            #[cfg(feature = "mem-arbitrator")]
            self.DetachMemArbitrator(exception);
        }
    }

    // remove 从 children map 删除 oldChild，并把 oldChild 已消费内存从父链扣回。
    fn remove(&mut self, oldChild: *mut Tracker) {
        let mut found = false;
        let label = unsafe { (*oldChild).label };
        {
            let mut children_guard = self.mu.children.lock().unwrap();
            if let Some(children_map) = children_guard.as_mut() {
                if let Some(children) = children_map.get_mut(&label) {
                    // Go 使用 slices.Delete；用 position/remove 表达同样的顺序删除。
                    if let Some(i) = children.iter().position(|child| *child == oldChild) {
                        children.remove(i);
                        if children.is_empty() {
                            children_map.remove(&label);
                        }
                        found = true;
                    }
                }
            }
        }
        if found {
            unsafe {
                (*oldChild).setParent(ptr::null_mut());
                self.Consume(-(*oldChild).BytesConsumed());
            }
        }
    }

    // ReplaceChild removes the old child specified in "oldChild" and add a new
    // child specified in "newChild". old child's memory consumption will be
    // removed and new child's memory consumption will be added.
    // ReplaceChild 替换子 tracker，并按新旧消费量差额更新父链。
    pub fn ReplaceChild(&mut self, oldChild: *mut Tracker, newChild: *mut Tracker) {
        if newChild.is_null() {
            self.remove(oldChild);
            return;
        }

        unsafe {
            if (*oldChild).label != (*newChild).label {
                self.remove(oldChild);
                (*newChild).AttachTo(self);
                return;
            }

            let mut newConsumed = (*newChild).BytesConsumed();
            (*newChild).setParent(self);

            let label = (*oldChild).label;
            {
                let mut children_guard = self.mu.children.lock().unwrap();
                if let Some(children_map) = children_guard.as_mut() {
                    if let Some(children) = children_map.get_mut(&label) {
                        for child in children.iter_mut() {
                            if *child != oldChild {
                                continue;
                            }

                            newConsumed -= (*oldChild).BytesConsumed();
                            (*oldChild).setParent(ptr::null_mut());
                            *child = newChild;
                            break;
                        }
                    }
                }
            }

            self.Consume(newConsumed);
        }
    }

    // Consume is used to consume a memory usage. "bytes" can be a negative value,
    // which means this is a memory release operation. When memory usage of a tracker
    // exceeds its bytesSoftLimit/bytesHardLimit, the tracker calls its action, so does each of its ancestors.
    // Consume 是 tracker 的核心路径：沿父链更新内存、仲裁预算、maxConsumed、metrics，并在超限时触发动作。
    pub fn Consume(&self, bs: i64) {
        if bs == 0 {
            return;
        }
        let mut rootExceed: *mut Tracker = ptr::null_mut();
        let mut rootExceedForSoftLimit: *mut Tracker = ptr::null_mut();
        let mut sessionRootTracker: *mut Tracker = ptr::null_mut();

        let mut tracker = self as *const Tracker as *mut Tracker;
        while !tracker.is_null() {
            unsafe {
                if (*tracker).IsRootTrackerOfSess {
                    sessionRootTracker = tracker;
                }
                #[cfg(feature = "mem-arbitrator")]
                if let Some(m) = (*tracker).MemArbitrator.as_mut() {
                    // Budget fast path: prefer small budget on positive consumption, fall back to big budget.
                    if bs > 0 {
                        if m.useBigBudget() {
                            if m.addBigBudgetUsed(bs) > m.bigBudgetGrowThreshold() {
                                m.growBigBudget();
                            }
                        } else {
                            // fast path for small budget
                            if m.addSmallBudget(bs) > m.small_limit {
                                m.intoBigBudget();
                            }
                        }
                    } else if m.useBigBudget() {
                        // delta <= 0 && use big budget
                        m.addBigBudgetUsed(bs);
                    } else {
                        // delta <= 0 && use small budget
                        m.addSmallBudget(bs);
                    }
                }

                let bytesConsumed = (*tracker).bytesConsumed.fetch_add(bs, Ordering::SeqCst) + bs;
                let bytesReleased = (*tracker).bytesReleased.load(Ordering::SeqCst);
                let limits = *(*tracker).bytesLimit.lock().unwrap();
                if bytesConsumed + bytesReleased >= limits.bytesHardLimit
                    && limits.bytesHardLimit > 0
                {
                    rootExceed = tracker;
                }
                if bytesConsumed + bytesReleased >= limits.bytesSoftLimit
                    && limits.bytesSoftLimit > 0
                {
                    rootExceedForSoftLimit = tracker;
                }

                loop {
                    let maxNow = (*tracker).maxConsumed.Load();
                    let consumed = (*tracker).bytesConsumed.load(Ordering::SeqCst);
                    if consumed > maxNow && !(*tracker).maxConsumed.CompareAndSwap(maxNow, consumed)
                    {
                        continue;
                    }
                    if (*tracker).label == LabelForGlobalAnalyzeMemory {
                        // `LabelForGlobalAnalyzeMemory` represents in-use memory, which should never be negative.
                        // Go 测试断言保留为 intest::Assert 占位，说明这里是分析全局内存不应为负的保护。
                        intest::Assert(
                            consumed >= 0,
                            format!("global analyze memory usage negative: {}", consumed),
                        );
                    }
                    if let Some(label) = MetricsTypes().get(&(*tracker).label) {
                        metrics::MemoryUsage
                            .WithLabelValues(label[0], label[1])
                            .Set(consumed as f64);
                    }
                    break;
                }

                tracker = (*tracker).getParent();
            }
        }

        // tryAction 清理已经 finished 的动作，再触发当前链头。Go 中用闭包捕获 mu 和 tracker。
        fn tryAction(mu: &actionMu, tracker: *mut Tracker) {
            let mut guard = mu.actionOnExceed.lock().unwrap();
            while guard.as_ref().map(|a| a.IsFinished()).unwrap_or(false) {
                let next = guard.as_mut().and_then(|a| a.GetFallback());
                *guard = next;
            }
            if let Some(action) = guard.as_mut() {
                unsafe {
                    action.Action(&mut *tracker);
                }
            }
        }

        if bs > 0 && !UsingGlobalMemArbitration() && !sessionRootTracker.is_null() {
            // Update the Top1 session
            // 全局内存仲裁未启用时，正向消费会尝试更新 Top1 session tracker，用于后续 kill。
            unsafe {
                let memUsage = (*sessionRootTracker).BytesConsumed();
                let limitSessMinSize = ServerMemoryLimitSessMinSize.Load();
                if (memUsage as u64) >= limitSessMinSize {
                    let mut oldTracker = MemUsageTop1Tracker.load(Ordering::SeqCst);
                    while oldTracker.is_null() || (*oldTracker).LessThan(sessionRootTracker) {
                        if MemUsageTop1Tracker
                            .compare_exchange(
                                oldTracker,
                                sessionRootTracker,
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                            )
                            .is_ok()
                        {
                            break;
                        }
                        oldTracker = MemUsageTop1Tracker.load(Ordering::SeqCst);
                    }
                }
            }
        }

        if bs > 0 && !sessionRootTracker.is_null() {
            // 正向消费后检查 session kill 信号；Go 中 HandleSignal 返回错误即 panic。
            unsafe {
                if let Some(killer) = (*sessionRootTracker).Killer.as_mut() {
                    if let Err(err) = killer.HandleSignal() {
                        panic!("{:?}", err);
                    }
                }
            }
        }

        if bs > 0 && !rootExceed.is_null() {
            unsafe {
                tryAction(&(*rootExceed).actionMuForHardLimit, rootExceed);
            }
        }

        if bs > 0 && !rootExceedForSoftLimit.is_null() {
            unsafe {
                tryAction(
                    &(*rootExceedForSoftLimit).actionMuForSoftLimit,
                    rootExceedForSoftLimit,
                );
            }
        }
    }

    // HandleKillSignal checks if a kill signal has been sent to the session root tracker.
    // If a kill signal is detected, it panics with the error returned by the signal handler.
    // HandleKillSignal 只沿父链寻找 session root，不做内存计数变更。
    pub fn HandleKillSignal(&self) {
        let mut sessionRootTracker: *mut Tracker = ptr::null_mut();
        let mut tracker = self as *const Tracker as *mut Tracker;
        while !tracker.is_null() {
            unsafe {
                if (*tracker).IsRootTrackerOfSess {
                    sessionRootTracker = tracker;
                }
                tracker = (*tracker).getParent();
            }
        }
        if !sessionRootTracker.is_null() {
            unsafe {
                if let Some(killer) = (*sessionRootTracker).Killer.as_mut() {
                    if let Err(err) = killer.HandleSignal() {
                        panic!("{:?}", err);
                    }
                }
            }
        }
    }

    // BufferedConsume is used to buffer memory usage and do late consume
    // not thread-safe, should be called in one goroutine
    // BufferedConsume 是单 goroutine 使用的延迟记账缓冲，超过阈值才调用 Consume。
    pub fn BufferedConsume(&self, bufferedMemSize: &mut i64, bytes: i64) {
        *bufferedMemSize += bytes;
        if *bufferedMemSize >= TrackMemWhenExceeds {
            self.Consume(*bufferedMemSize);
            *bufferedMemSize = 0;
        }
    }

    // Release is used to release memory tracked, track the released memory until GC triggered if needed
    // If you want your track to be GC-aware, please use Release(bytes) instead of Consume(-bytes), and pass the memory size of the real object.
    // Only Analyze is integrated with Release so far.
    // Release 在 GC-aware 开启时先记录 released，等 finalizer 异步回调后再扣除 released 统计。
    pub fn Release(&self, bytes: i64) {
        if bytes == 0 {
            return;
        }

        let mut tracker = self as *const Tracker as *mut Tracker;
        while !tracker.is_null() {
            unsafe {
                if (*tracker).shouldRecordRelease() {
                    // Go 使用 fake ref，避免真实对象因为 finalizer 闭包重新变成 reachable。
                    // Rust 没有直接等价的 runtime.SetFinalizer；这里用注释保留异步 GC 回调语义。
                    (*tracker).recordRelease(bytes);
                    self.Consume(-bytes);
                    // Rust has no Go-style object finalizer. Complete the released
                    // accounting at the ownership boundary represented by Release.
                    (*tracker).release(bytes);
                    return;
                }
                tracker = (*tracker).getParent();
            }
        }
        // Go defer t.Consume(-bytes) 在所有分支结束时执行；在这里显式展开。
        self.Consume(-bytes);
    }

    // BufferedRelease is used to buffer memory release and do late release
    // not thread-safe, should be called in one goroutine
    // BufferedRelease 是 Release 的缓冲版本，同样要求单 goroutine 调用。
    pub fn BufferedRelease(&self, bufferedMemSize: &mut i64, bytes: i64) {
        *bufferedMemSize += bytes;
        if *bufferedMemSize >= TrackMemWhenExceeds {
            self.Release(*bufferedMemSize);
            *bufferedMemSize = 0;
        }
    }

    // shouldRecordRelease 判断当前 tracker 是否需要 GC-aware released 统计，目前只覆盖全局 Analyze 内存。
    fn shouldRecordRelease(&self) -> bool {
        EnableGCAwareMemoryTrack.Load() && self.label == LabelForGlobalAnalyzeMemory
    }

    // recordRelease 沿父链累加 released 统计，并更新 metrics 的 released label。
    fn recordRelease(&self, bytes: i64) {
        let mut tracker = self as *const Tracker as *mut Tracker;
        while !tracker.is_null() {
            unsafe {
                let bytesReleased =
                    (*tracker).bytesReleased.fetch_add(bytes, Ordering::SeqCst) + bytes;
                if let Some(label) = MetricsTypes().get(&(*tracker).label) {
                    metrics::MemoryUsage
                        .WithLabelValues(label[0], label[2])
                        .Set(bytesReleased as f64);
                }
                tracker = (*tracker).getParent();
            }
        }
    }

    // release 是 finalizer 异步触发后的反向操作，沿父链扣减 bytesReleased。
    fn release(&self, bytes: i64) {
        let mut tracker = self as *const Tracker as *mut Tracker;
        while !tracker.is_null() {
            unsafe {
                let bytesReleased =
                    (*tracker).bytesReleased.fetch_add(-bytes, Ordering::SeqCst) - bytes;
                if let Some(label) = MetricsTypes().get(&(*tracker).label) {
                    metrics::MemoryUsage
                        .WithLabelValues(label[0], label[2])
                        .Set(bytesReleased as f64);
                }
                tracker = (*tracker).getParent();
            }
        }
    }

    // BytesConsumed returns the consumed memory usage value in bytes.
    // BytesConsumed 返回当前 tracker 自身的 consumed 原子值。
    pub fn BytesConsumed(&self) -> i64 {
        self.bytesConsumed.load(Ordering::SeqCst)
    }

    // BytesReleased returns the released memory value in bytes.
    // BytesReleased 返回 GC-aware 路径暂存的 released 原子值。
    pub fn BytesReleased(&self) -> i64 {
        self.bytesReleased.load(Ordering::SeqCst)
    }

    // MaxConsumed returns max number of bytes consumed during execution.
    // Note: Don't make this method return -1 for special meanings in the future. Because binary plan has used -1 to
    // distinguish between "0 bytes" and "N/A". ref: binaryOpFromFlatOp()
    // MaxConsumed 返回执行期间峰值，保留 Go 注释中的二进制计划兼容约束。
    pub fn MaxConsumed(&self) -> i64 {
        self.maxConsumed.Load()
    }

    // ResetMaxConsumed should be invoked before executing a new statement in a session.
    // ResetMaxConsumed 在新 statement 前把峰值重置为当前 consumed，而不是清零。
    pub fn ResetMaxConsumed(&self) {
        self.maxConsumed.Store(self.BytesConsumed());
    }

    // SearchTrackerWithoutLock searches the specific tracker under this tracker without lock.
    // SearchTrackerWithoutLock 按 Go 语义不加锁；调用者必须确保外层同步。
    pub fn SearchTrackerWithoutLock(&self, label: i32) -> *mut Tracker {
        if self.label == label {
            return self as *const Tracker as *mut Tracker;
        }
        let children_guard = self.mu.children.lock().unwrap();
        if let Some(children_map) = children_guard.as_ref() {
            if let Some(children) = children_map.get(&label) {
                if !children.is_empty() {
                    return children[0];
                }
            }
        }
        ptr::null_mut()
    }

    // SearchTrackerConsumedMoreThanNBytes searches the specific tracker that consumes more than NBytes.
    // SearchTrackerConsumedMoreThanNBytes 加锁扫描直接子节点，返回消费量超过阈值的 tracker 列表。
    pub fn SearchTrackerConsumedMoreThanNBytes(&self, limit: i64) -> Vec<*mut Tracker> {
        let mut res = Vec::new();
        let children_guard = self.mu.children.lock().unwrap();
        if let Some(children_map) = children_guard.as_ref() {
            for childSlice in children_map.values() {
                for tracker in childSlice {
                    unsafe {
                        if (**tracker).BytesConsumed() > limit {
                            res.push(*tracker);
                        }
                    }
                }
            }
        }
        res
    }

    // String returns the string representation of this Tracker tree.
    // String 输出 tracker 树的文本表示；保持 Go 中以换行开头的格式。
    pub fn String(&self) -> String {
        let mut buffer = String::from("\n");
        self.toString("", &mut buffer);
        buffer
    }

    // toString 递归格式化当前节点和子树；children label 先排序以获得稳定输出。
    fn toString(&self, indent: &str, buffer: &mut String) {
        buffer.push_str(&format!("{}\"{}\"{{\n", indent, self.label));
        let bytesLimit = self.GetBytesLimit();
        if bytesLimit > 0 {
            buffer.push_str(&format!(
                "{}  \"quota\": {}\n",
                indent,
                self.FormatBytes(bytesLimit)
            ));
        }
        buffer.push_str(&format!(
            "{}  \"consumed\": {}\n",
            indent,
            self.FormatBytes(self.BytesConsumed())
        ));

        let children_guard = self.mu.children.lock().unwrap();
        let mut labels: Vec<i32> = children_guard
            .as_ref()
            .map(|children| children.keys().copied().collect())
            .unwrap_or_default();
        labels.sort();
        if let Some(children_map) = children_guard.as_ref() {
            for label in labels {
                if let Some(children) = children_map.get(&label) {
                    for child in children {
                        unsafe {
                            (**child).toString(&(indent.to_owned() + "  "), buffer);
                        }
                    }
                }
            }
        }
        buffer.push_str(&format!("{}}}\n", indent));
    }

    // FormatBytes uses to format bytes, this function will prune precision before format bytes.
    // Tracker::FormatBytes 是包级 FormatBytes 的方法包装，保留 Go 接收者形状。
    pub fn FormatBytes(&self, numBytes: i64) -> String {
        FormatBytes(numBytes)
    }

    // LessThan indicates whether t byteConsumed is less than t2 byteConsumed.
    // LessThan 处理 nil 语义：nil 小于非 nil，非 nil 大于 nil。
    pub fn LessThan(&self, t2: *mut Tracker) -> bool {
        if t2.is_null() {
            return false;
        }
        unsafe { self.BytesConsumed() < (*t2).BytesConsumed() }
    }
}

// BytesToString converts the memory consumption to a readable string.
// BytesToString 使用粗粒度单位输出，不裁剪小数精度；FormatBytes 会在此基础上选择精度。
/// 将字节数格式化为带单位的字符串（近似 Go BytesToString）。
pub fn BytesToString(numBytes: i64) -> String {
    let gb = numBytes as f64 / byteSizeGB as f64;
    if gb > 1.0 {
        return format!("{} GB", gb);
    }

    let mb = numBytes as f64 / byteSizeMB as f64;
    if mb > 1.0 {
        return format!("{} MB", mb);
    }

    let kb = numBytes as f64 / byteSizeKB as f64;
    if kb > 1.0 {
        return format!("{} KB", kb);
    }

    format!("{} Bytes", numBytes)
}

// FormatBytes uses to format bytes, this function will prune precision before format bytes.
// FormatBytes 先选择最大合适单位，再按是否整除和数值大小决定小数位数。
/// 格式化字节数（带修剪），供展示与测试断言。
pub fn FormatBytes(numBytes: i64) -> String {
    if numBytes <= byteSizeKB {
        return BytesToString(numBytes);
    }
    let (unit, unitStr) = getByteUnit(numBytes);
    if unit == byteSize {
        return BytesToString(numBytes);
    }
    let v = numBytes as f64 / unit as f64;
    let mut decimal = 1;
    if numBytes % unit == 0 {
        decimal = 0;
    } else if v < 10.0 {
        decimal = 2;
    }
    format!("{:.*} {}", decimal as usize, v, unitStr)
}

// getByteUnit 根据字节数选择 GB/MB/KB/Bytes 单位；byteSize* 常量来自同 package 的 utils.go。
fn getByteUnit(b: i64) -> (i64, &'static str) {
    if b > byteSizeGB {
        (byteSizeGB, "GB")
    } else if b > byteSizeMB {
        (byteSizeMB, "MB")
    } else if b > byteSizeKB {
        (byteSizeKB, "KB")
    } else {
        (byteSize, "Bytes")
    }
}

impl Tracker {
    // AttachToGlobalTracker attach the tracker to the global tracker
    // AttachToGlobalTracker should be called at the initialization for the session executor's tracker
    // AttachToGlobalTracker 只允许挂到 global tracker；global tracker 不维护 children，只更新消费量父链。
    pub fn AttachToGlobalTracker(&mut self, globalTracker: *mut Tracker) {
        if globalTracker.is_null() {
            return;
        }
        unsafe {
            if !(*globalTracker).isGlobal {
                panic!("Attach to a non-GlobalTracker");
            }
            let parent = self.getParent();
            if !parent.is_null() {
                if (*parent).isGlobal {
                    (*parent).Consume(-self.BytesConsumed());
                } else {
                    (*parent).remove(self);
                }
            }
            self.setParent(globalTracker);
            (*globalTracker).Consume(self.BytesConsumed());
        }
    }

    // DetachFromGlobalTracker detach itself from its parent
    // Note that only the parent of this tracker is Global Tracker could call this function
    // Otherwise it should use Detach
    // DetachFromGlobalTracker 只服务父节点是 global tracker 的情况，负责扣回消费量并清父指针。
    pub fn DetachFromGlobalTracker(&mut self) {
        let parent = self.getParent();
        if parent.is_null() {
            return;
        }
        unsafe {
            if !(*parent).isGlobal {
                panic!("Detach from a non-GlobalTracker");
            }
            (*parent).Consume(-self.BytesConsumed());
            self.setParent(ptr::null_mut());
        }
    }

    // ReplaceBytesUsed replace bytesConsume for the tracker
    // ReplaceBytesUsed 用目标值和当前值的差额调用 Consume，确保父链同步更新。
    pub fn ReplaceBytesUsed(&self, bytes: i64) {
        self.Consume(bytes - self.BytesConsumed());
    }

    // Reset detach the tracker from the old parent and clear the old children. The label and byteLimit would not be reset.
    // Reset 保留 label 和 byteLimit，只清父子关系和已使用字节数。
    pub fn Reset(&mut self) {
        self.Detach();
        self.ReplaceBytesUsed(0);
        *self.mu.children.lock().unwrap() = None;
    }

    // getParent 读取 atomic parent 指针，对应 Go parent.Load。
    fn getParent(&self) -> *mut Tracker {
        self.parent.load(Ordering::SeqCst)
    }

    // setParent 写入 atomic parent 指针，对应 Go parent.Store。
    fn setParent(&self, parent: *mut Tracker) {
        self.parent.store(parent, Ordering::SeqCst);
    }

    // CountAllChildrenMemUse return memory used tree for the tracker
    // CountAllChildrenMemUse 构造 family-tree-name 到 consumed 的映射，用于观察整棵子树内存。
    pub fn CountAllChildrenMemUse(&self) -> HashMap<String, i64> {
        let mut trackerMemUseMap = HashMap::with_capacity(1024);
        countChildMem(self, String::new(), &mut trackerMemUseMap);
        trackerMemUseMap
    }

    // GetChildrenForTest returns children trackers
    // GetChildrenForTest 加锁复制直接 children，供测试断言树结构。
    pub fn GetChildrenForTest(&self) -> Vec<*mut Tracker> {
        let mut trackers = Vec::new();
        let children_guard = self.mu.children.lock().unwrap();
        if let Some(children_map) = children_guard.as_ref() {
            for list in children_map.values() {
                trackers.extend(list.iter().copied());
            }
        }
        trackers
    }
}

// countChildMem 递归遍历 tracker 树，把路径名拼成 "[parent] <- [child]" 形式。
fn countChildMem(
    t: &Tracker,
    mut familyTreeName: String,
    trackerMemUseMap: &mut HashMap<String, i64>,
) {
    if !familyTreeName.is_empty() {
        familyTreeName.push_str(" <- ");
    }
    familyTreeName.push_str(&format!("[{}]", t.Label()));
    *trackerMemUseMap.entry(familyTreeName.clone()).or_insert(0) += t.BytesConsumed();
    let children_guard = t.mu.children.lock().unwrap();
    if let Some(children_map) = children_guard.as_ref() {
        for sli in children_map.values() {
            for tracker in sli {
                unsafe {
                    countChildMem(&**tracker, familyTreeName.clone(), trackerMemUseMap);
                }
            }
        }
    }
}

// Tracker label 常量保持 Go 的负数编码，用于在内存树和 metrics 中区分来源模块。
/// Tracker 标签常量：标识 SQL/执行器/缓存等内存归属（负值保留）。
pub const LabelForSQLText: i32 = -1;
pub const LabelForIndexWorker: i32 = -2;
pub const LabelForInnerList: i32 = -3;
pub const LabelForInnerTable: i32 = -4;
pub const LabelForOuterTable: i32 = -5;
pub const LabelForCoprocessor: i32 = -6;
pub const LabelForChunkList: i32 = -7;
pub const LabelForGlobalSimpleLRUCache: i32 = -8;
pub const LabelForChunkDataInDiskByRows: i32 = -9;
pub const LabelForRowContainer: i32 = -10;
pub const LabelForGlobalStorage: i32 = -11;
pub const LabelForGlobalMemory: i32 = -12;
pub const LabelForBuildSideResult: i32 = -13;
pub const LabelForRowChunks: i32 = -14;
pub const LabelForStatsCache: i32 = -15;
pub const LabelForOuterList: i32 = -16;
pub const LabelForApplyCache: i32 = -17;
pub const LabelForSimpleTask: i32 = -18;
pub const LabelForCTEStorage: i32 = -19;
pub const LabelForIndexJoinInnerWorker: i32 = -20;
pub const LabelForIndexJoinOuterWorker: i32 = -21;
pub const LabelForBindCache: i32 = -22;
pub const LabelForNonTransactionalDML: i32 = -23;
pub const LabelForAnalyzeMemory: i32 = -24;
pub const LabelForGlobalAnalyzeMemory: i32 = -25;
pub const LabelForPreparedPlanCache: i32 = -26;
pub const LabelForSession: i32 = -27;
pub const LabelForMemDB: i32 = -28;
pub const LabelForCursorFetch: i32 = -29;
pub const LabelForChunkDataInDiskByChunks: i32 = -30;
pub const LabelForSortPartition: i32 = -31;
pub const LabelForHashTableInHashJoinV2: i32 = -32;

// MetricsTypes is used to get label for metrics
// string[0] is LblModule, string[1] is heap-in-use type, string[2] is released type
// MetricsTypes 在 Go 中是 map[int][]string；用函数返回 map，避免展开全局初始化细节。
/// 标签到 metrics 类型三元组的映射表。
pub fn MetricsTypes() -> HashMap<i32, [&'static str; 3]> {
    HashMap::from([(
        LabelForGlobalAnalyzeMemory,
        ["analyze", "inuse", "released"],
    )])
}

#[cfg(feature = "mem-arbitrator")]
const memArbitratorStateSmallBudget: i32 = 0;
#[cfg(feature = "mem-arbitrator")]
const memArbitratorStateBigBudget: i32 = 2;
#[cfg(feature = "mem-arbitrator")]
const memArbitratorStateDown: i32 = 3;

#[cfg(feature = "mem-arbitrator")]
struct TrackerArbitrateHelper {
    killer: Option<Arc<sqlkiller::SQLKiller>>,
    heap_inuse: AtomicI64,
    finished: AtomicBool,
}

#[cfg(feature = "mem-arbitrator")]
impl ArbitrateHelper for TrackerArbitrateHelper {
    fn Stop(&self, reason: ArbitratorStopReason) -> bool {
        if let Some(killer) = &self.killer {
            killer.SendKillSignalWithKillEventReason(
                sqlkiller::KilledByMemArbitrator,
                reason.String(),
            );
        }
        true
    }

    fn HeapInuse(&self) -> i64 {
        self.heap_inuse.load(Ordering::Acquire)
    }

    fn Finish(&self) {
        self.finished.store(true, Ordering::Release);
    }
}

#[cfg(feature = "mem-arbitrator")]
pub struct awaitAlloc {
    pub TotalDur: atomicutil::Int64,
    pub StartUtime: i64,
    pub Size: i64,
}

#[cfg(feature = "mem-arbitrator")]
pub struct memArbitrator {
    pub MemArbitrator: Arc<MemArbitrator>,
    ctx: Arc<ArbitrationContext>,
    helper: Arc<TrackerArbitrateHelper>,
    small_budget: Arc<TrackedConcurrentBudget>,
    small_used: AtomicI64,
    small_limit: i64,
    big_budget: ConcurrentBudget,
    big_used: AtomicI64,
    big_grow_threshold: AtomicI64,
    root: Option<RootPoolHandle>,
    use_big: AtomicBool,
    uid: u64,
    digest_id: u64,
    reserve_size: i64,
    previous_max: i64,
    is_internal: bool,
    state: AtomicI32,
    pub AwaitAlloc: awaitAlloc,
}

#[cfg(feature = "mem-arbitrator")]
impl memArbitrator {
    fn useBigBudget(&self) -> bool {
        self.use_big.load(Ordering::Acquire)
    }

    fn smallBudget(&self) -> *mut TrackedConcurrentBudget {
        Arc::as_ptr(&self.small_budget) as *mut TrackedConcurrentBudget
    }

    fn smallBudgetUsed(&self) -> i64 {
        self.small_used.load(Ordering::Acquire)
    }

    fn addSmallBudget(&self, delta: i64) -> i64 {
        let _ = self
            .MemArbitrator
            .ConsumeQuotaFromAwaitFreePool(self.uid, delta);
        self.MemArbitrator
            .ReportHeapInuseToAwaitFreePool(self.uid, delta);
        self.helper.heap_inuse.fetch_add(delta, Ordering::AcqRel);
        self.small_used.fetch_add(delta, Ordering::AcqRel) + delta
    }

    fn cleanSmallBudget(&self) -> i64 {
        let used = self.small_used.swap(0, Ordering::AcqRel);
        if used != 0 {
            let _ = self
                .MemArbitrator
                .ConsumeQuotaFromAwaitFreePool(self.uid, -used);
            self.MemArbitrator
                .ReportHeapInuseToAwaitFreePool(self.uid, -used);
        }
        used
    }

    fn bigBudgetGrowThreshold(&self) -> i64 {
        self.big_grow_threshold.load(Ordering::Acquire)
    }

    fn bigBudgetCap(&self) -> i64 {
        self.big_budget.capacity()
    }

    fn bigBudgetUsed(&self) -> i64 {
        self.big_used.load(Ordering::Acquire)
    }

    fn addBigBudgetUsed(&self, delta: i64) -> i64 {
        self.helper.heap_inuse.fetch_add(delta, Ordering::AcqRel);
        self.big_used.fetch_add(delta, Ordering::AcqRel) + delta
    }

    fn approxUnixTimeSec(&self) -> i64 {
        self.MemArbitrator.approxUnixTimeSec()
    }

    fn intoBigBudget(&mut self) -> bool {
        if self.useBigBudget() {
            return false;
        }
        let Ok(root) = self.MemArbitrator.EmplaceRootPool(self.uid) else {
            return false;
        };
        if !self
            .MemArbitrator
            .RestartEntryByContext(root, self.ctx.clone())
        {
            return false;
        }

        let small_used = self.smallBudgetUsed().max(0);
        let initial = self
            .reserve_size
            .max(self.previous_max)
            .max(small_used)
            .max(self.MemArbitrator.SuggestPoolInitCap());
        if initial > 0 && self.MemArbitrator.RequestQuota(root, initial) != ArbitrateOk {
            let _ = self.MemArbitrator.ResetRootPoolByID(self.uid, 0, false);
            return false;
        }

        self.big_budget.Reserve(initial);
        self.big_used.store(small_used, Ordering::Release);
        self.big_grow_threshold
            .store((initial * 95 / 100).max(small_used), Ordering::Release);
        self.cleanSmallBudget();
        self.root = Some(root);
        self.use_big.store(true, Ordering::Release);
        self.state
            .store(memArbitratorStateBigBudget, Ordering::Release);
        true
    }

    fn growBigBudget(&mut self) {
        let Some(root) = self.root else {
            return;
        };
        let used = self.bigBudgetUsed();
        if used <= self.bigBudgetGrowThreshold() {
            return;
        }
        let capacity = self.bigBudgetCap();
        let target = ((used * 2_783) >> 10)
            .max(used)
            .min(capacity + self.MemArbitrator.PoolAllocProfile().MaxPoolAllocUnit);
        let extra = (target - capacity).max(used - capacity).max(0);
        if extra == 0 {
            return;
        }

        self.AwaitAlloc.StartUtime = now_unix_nano();
        self.AwaitAlloc.Size = extra;
        if self.MemArbitrator.RequestQuota(root, extra) == ArbitrateOk {
            self.big_budget.Reserve(extra);
            self.big_grow_threshold.store(
                (self.bigBudgetCap() * 95 / 100).max(used),
                Ordering::Release,
            );
        }
        let duration = now_unix_nano() - self.AwaitAlloc.StartUtime;
        self.AwaitAlloc.TotalDur.Add(duration.max(0));
        self.AwaitAlloc.StartUtime = 0;
        self.AwaitAlloc.Size = 0;
    }

    fn reset(&mut self, exception: bool, max_consumed: i64) -> bool {
        if self.state.swap(memArbitratorStateDown, Ordering::AcqRel) == memArbitratorStateDown {
            return false;
        }
        self.cleanSmallBudget();
        if !exception {
            self.MemArbitrator.UpdateDigestProfileCache(
                self.digest_id,
                max_consumed,
                self.approxUnixTimeSec(),
            );
        }
        if self.root.is_some() {
            self.MemArbitrator
                .ResetRootPoolByID(self.uid, max_consumed, !exception);
        }
        true
    }

    pub fn Finish(&self) {
        self.helper.Finish();
    }

    pub fn Stop(&self, reason: ArbitratorStopReason) -> bool {
        self.helper.Stop(reason)
    }

    pub fn HeapInuse(&self) -> i64 {
        self.helper.HeapInuse()
    }
}

#[cfg(feature = "mem-arbitrator")]
impl Tracker {
    pub fn MemArbitration(&self) -> Duration {
        self.MemArbitrator.as_ref().map_or(Duration::ZERO, |m| {
            Duration::from_nanos(m.AwaitAlloc.TotalDur.Load().max(0) as u64)
        })
    }

    pub fn WaitArbitrate(&self) -> (SystemTime, i64) {
        self.MemArbitrator
            .as_ref()
            .map_or((SystemTime::UNIX_EPOCH, 0), |m| {
                (
                    unix_nano_to_time(m.AwaitAlloc.StartUtime),
                    m.AwaitAlloc.Size,
                )
            })
    }

    pub fn DetachMemArbitrator(&mut self, exception: bool) -> bool {
        let max_consumed = self.MaxConsumed();
        let Some(m) = self.MemArbitrator.as_mut() else {
            return false;
        };
        m.reset(exception, max_consumed)
    }

    pub fn InitMemArbitratorForTest(&mut self) -> bool {
        self.InitMemArbitrator(
            GlobalMemArbitrator(),
            None,
            "",
            ArbitrationPriorityMedium,
            false,
            0,
            false,
        )
    }

    pub fn InitMemArbitrator(
        &mut self,
        core: Option<Arc<MemArbitrator>>,
        killer: Option<Box<sqlkiller::SQLKiller>>,
        digest_key: &str,
        mem_priority: ArbitrationPriority,
        wait_averse: bool,
        explicit_reserve_size: i64,
        is_internal: bool,
    ) -> bool {
        let Some(core) = core else {
            return false;
        };
        if let Some(mut previous) = self.MemArbitrator.take() {
            previous.reset(true, 0);
        }

        let killer: Option<Arc<sqlkiller::SQLKiller>> = killer.map(Arc::from);
        let cancel = killer
            .as_ref()
            .map(|killer| CancelReceiver::from_kill_event(killer.GetKillEventChan()))
            .unwrap_or_else(CancelReceiver::none);
        let helper = Arc::new(TrackerArbitrateHelper {
            killer,
            heap_inuse: AtomicI64::new(0),
            finished: AtomicBool::new(false),
        });
        let context = crate::NewArbitrationContext(
            cancel,
            Some(helper.clone()),
            mem_priority,
            wait_averse,
            true,
        );
        let uid = self.SessionID.Load();
        let digest_id = HashStr(digest_key);
        let previous_max = if explicit_reserve_size == 0 && !digest_key.is_empty() {
            core.GetDigestProfileCache(digest_id, core.approxUnixTimeSec())
                .unwrap_or(0)
        } else {
            0
        };
        let small_limit = core.PoolAllocProfile().SmallPoolLimit;
        let small_budget = core.GetAwaitFreeBudgets(uid);
        let mut session = Box::new(memArbitrator {
            MemArbitrator: core,
            ctx: context,
            helper,
            small_budget,
            small_used: AtomicI64::new(0),
            small_limit,
            big_budget: ConcurrentBudget::default(),
            big_used: AtomicI64::new(0),
            big_grow_threshold: AtomicI64::new(0),
            root: None,
            use_big: AtomicBool::new(false),
            uid,
            digest_id,
            reserve_size: explicit_reserve_size,
            previous_max,
            is_internal,
            state: AtomicI32::new(memArbitratorStateSmallBudget),
            AwaitAlloc: awaitAlloc {
                TotalDur: atomicutil::Int64::new(0),
                StartUtime: 0,
                Size: 0,
            },
        });
        if explicit_reserve_size > 0 || previous_max > small_limit {
            session.intoBigBudget();
        }
        self.MemArbitrator = Some(session);
        true
    }
}

// unix_nano_to_time、now_unix_nano 和 max_many 是 Rust 辅助函数，对应 Go time.Now/UnixNano 和内建 max。
fn unix_nano_to_time(ns: i64) -> SystemTime {
    if ns <= 0 {
        return SystemTime::UNIX_EPOCH;
    }
    SystemTime::UNIX_EPOCH + Duration::from_nanos(ns as u64)
}

fn now_unix_nano() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

fn max_many(values: &[i64]) -> i64 {
    values.iter().copied().max().unwrap_or(0)
}
