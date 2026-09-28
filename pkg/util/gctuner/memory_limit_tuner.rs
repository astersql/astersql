// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 运行时 memory limit 调谐器。
//
// 对应 Go `util/gctuner` 的 MemoryLimitTuner：按 `ServerMemoryLimit * percentage`
// 设置进程内存上限。当预计下次 GC 后堆占用仍会超过当前 limit 时，短暂抬升到
// `fallbackPercentage`（110%），给回收窗口，再复位到配置比例。
//
// 两阶段判定：连续两次“触达 limit”才进入调整窗口，避免单次抖动误触发。

use crate::finalizer::{Finalizer, newFinalizer};
use crate::mem::readMemoryInuse;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::thread;
use std::time::{Duration, SystemTime};
use task_memory::{global_arbitrator, tracker};
use task_util::gogc;

/// 调整窗口内临时抬升的比例（110%），给 GC 回收留余量。
pub const fallbackPercentage: f64 = 1.1;
/// 默认复位间隔：临时 fallback 持续多久后恢复配置比例。
const DEFAULT_RESET_INTERVAL: Duration = Duration::from_secs(60);

/// 进程启动时记录的初始 Go memory limit（禁用调整时回退到此值）。
pub static initGOMemoryLimitValue: AtomicI64 = AtomicI64::new(i64::MAX);
/// 当前生效的 runtime memory limit；负参查询、非负参交换。
static RUNTIME_MEMORY_LIMIT: AtomicI64 = AtomicI64::new(i64::MAX);
/// 进行中的复位后台任务计数（测试等待其退出）。
static MEMORY_GOROUTINE_COUNT: AtomicI64 = AtomicI64::new(0);

/// 设置或查询 runtime memory limit；`limit < 0` 表示只读当前值。
fn setMemoryLimit(limit: i64) -> i64 {
    if limit < 0 {
        return RUNTIME_MEMORY_LIMIT.load(Ordering::SeqCst);
    }
    RUNTIME_MEMORY_LIMIT.swap(limit, Ordering::SeqCst)
}

/// 返回当前生效的 memory limit（不修改）。
pub fn currentMemoryLimit() -> i64 {
    setMemoryLimit(-1)
}

/// 用 `AtomicU64` 存 f64 位模式，提供 SeqCst 的浮点原子读写。
struct AtomicF64(AtomicU64);

impl AtomicF64 {
    const fn new(value: f64) -> Self {
        Self(AtomicU64::new(value.to_bits()))
    }

    fn load(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::SeqCst))
    }

    fn store(&self, value: f64) {
        self.0.store(value.to_bits(), Ordering::SeqCst);
    }
}

/// Memory limit 调谐器：由 finalizer 周期性调用 `tuning`，并在触达 limit 时启动复位 worker。
pub struct MemoryLimitTuner {
    /// 驱动周期性 `tuning` 的 finalizer。
    finalizer: Mutex<Arc<Finalizer>>,
    /// 是否已设置过有效（非 MAX）的 memory limit。
    isValidValueSet: AtomicBool,
    /// 配置的 limit 占 `ServerMemoryLimit` 的比例。
    percentage: AtomicF64,
    /// 是否正处于 fallback→复位调整窗口。
    adjustPercentageInProgress: AtomicBool,
    /// 进入调整前的服务器内存上限快照。
    serverMemLimitBeforeAdjust: AtomicU64,
    /// 进入调整前的 percentage 快照。
    percentageBeforeAdjust: AtomicF64,
    /// 上次 GC 是否因 memory limit 触发（两阶段判定用）。
    nextGCTriggeredByMemoryLimit: AtomicBool,
    /// 禁用调整的嵌套计数（>0 时强制用初始 limit）。
    adjustDisabled: AtomicI64,
    /// 串行化 `tuning` / `UpdateMemoryLimit`。
    tuningLock: Mutex<()>,
    /// fallback 后恢复配置比例前的等待时长。
    resetInterval: Duration,
    /// 供后台线程 upgrade 自身，避免强引用环。
    selfWeak: Weak<MemoryLimitTuner>,
}

/// Go 风格类型别名。
pub type memoryLimitTuner = MemoryLimitTuner;

impl MemoryLimitTuner {
    /// 使用默认复位间隔构造全局风格实例。
    pub fn new() -> Arc<Self> {
        Self::withResetInterval(DEFAULT_RESET_INTERVAL)
    }

    /// 指定复位间隔构造；finalizer 回调中 upgrade weak 后调用 `tuning`。
    pub fn withResetInterval(reset_interval: Duration) -> Arc<Self> {
        Arc::new_cyclic(|weak: &Weak<MemoryLimitTuner>| {
            let callback_weak = weak.clone();
            Self {
                finalizer: Mutex::new(newFinalizer(Box::new(move || {
                    if let Some(tuner) = callback_weak.upgrade() {
                        tuner.tuning();
                    }
                }))),
                isValidValueSet: AtomicBool::new(false),
                percentage: AtomicF64::new(0.0),
                adjustPercentageInProgress: AtomicBool::new(false),
                serverMemLimitBeforeAdjust: AtomicU64::new(0),
                percentageBeforeAdjust: AtomicF64::new(0.0),
                nextGCTriggeredByMemoryLimit: AtomicBool::new(false),
                adjustDisabled: AtomicI64::new(0),
                tuningLock: Mutex::new(()),
                resetInterval: reset_interval,
                selfWeak: weak.clone(),
            }
        })
    }

    /// 禁用自动调整并恢复到初始 Go memory limit。
    pub fn DisableAdjustMemoryLimit(&self) {
        self.adjustDisabled.fetch_add(1, Ordering::SeqCst);
        setMemoryLimit(initGOMemoryLimitValue.load(Ordering::SeqCst));
    }

    /// 重新启用自动调整并立即按当前 percentage 刷新 limit。
    pub fn EnableAdjustMemoryLimit(&self) {
        self.adjustDisabled.fetch_sub(1, Ordering::SeqCst);
        self.UpdateMemoryLimit();
        resetGlobalArbitratorLimit();
    }

    /// 核心调谐：若 `heap_inuse * (1+GOGC/100)` 超过当前 limit，进入两阶段调整。
    pub fn tuning(&self) {
        let _guard = self.tuningLock.lock().expect("memory tuner lock poisoned");
        // 未配置有效 limit，或全局内存仲裁接管时，跳过本地调谐。
        if !self.isValidValueSet.load(Ordering::SeqCst)
            || global_arbitrator::UsingGlobalMemArbitration()
        {
            return;
        }

        let heap_inuse = readMemoryInuse();
        // GOGC 语义：下次 GC 触发时堆约为当前占用的 (100+GOGC)%。
        let ratio = f64::from(100 + gogc::GetGOGC()) / 100.0;
        if heap_inuse as f64 * ratio > currentMemoryLimit() as f64 {
            // 第二次连续触达才真正进入调整：抬升 fallback 并启动复位 worker。
            if self.nextGCTriggeredByMemoryLimit.load(Ordering::SeqCst)
                && self
                    .adjustPercentageInProgress
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                self.serverMemLimitBeforeAdjust
                    .store(tracker::ServerMemoryLimit.Load(), Ordering::SeqCst);
                self.percentageBeforeAdjust.store(self.GetPercentage());
                self.startResetWorker();
            }
            self.nextGCTriggeredByMemoryLimit
                .store(true, Ordering::SeqCst);
            tracker::TriggerMemoryLimitGC.Store(true);
        } else {
            self.nextGCTriggeredByMemoryLimit
                .store(false, Ordering::SeqCst);
            tracker::TriggerMemoryLimitGC.Store(false);
        }
    }

    /// 后台：立即设为 fallbackPercentage，sleep 后再恢复配置 percentage。
    fn startResetWorker(&self) {
        let weak = self.selfWeak.clone();
        let reset_interval = self.resetInterval;
        MEMORY_GOROUTINE_COUNT.fetch_add(1, Ordering::SeqCst);
        thread::spawn(move || {
            /// RAII：线程退出时递减进行中任务计数。
            struct CountGuard;
            impl Drop for CountGuard {
                fn drop(&mut self) {
                    MEMORY_GOROUTINE_COUNT.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _count_guard = CountGuard;
            let Some(tuner) = weak.upgrade() else {
                return;
            };

            tracker::MemoryLimitGCLast.Store(SystemTime::now());
            tracker::MemoryLimitGCTotal.Add(1);
            setMemoryLimit(tuner.calcMemoryLimit(fallbackPercentage));
            thread::sleep(reset_interval);
            setMemoryLimit(tuner.calcMemoryLimit(tuner.GetPercentage()));
            tuner
                .adjustPercentageInProgress
                .store(false, Ordering::SeqCst);
            resetGlobalArbitratorLimit();
        });
    }

    /// 启动或重新启动 finalizer 驱动。
    pub fn Start(&self) {
        let weak = self.selfWeak.clone();
        let replacement = newFinalizer(Box::new(move || {
            if let Some(tuner) = weak.upgrade() {
                tuner.tuning();
            }
        }));
        let mut finalizer = self
            .finalizer
            .lock()
            .expect("memory tuner finalizer lock poisoned");
        finalizer.stop();
        *finalizer = replacement;
    }

    /// 手动跑一轮 finalizer（测试用）。
    pub fn runFinalizer(&self) -> bool {
        self.finalizer
            .lock()
            .expect("memory tuner finalizer lock poisoned")
            .run()
    }

    /// 停止 finalizer，后续不再调谐。
    pub fn Stop(&self) {
        self.finalizer
            .lock()
            .expect("memory tuner finalizer lock poisoned")
            .stop();
    }

    /// 设置 limit 占服务器内存上限的比例。
    pub fn SetPercentage(&self, percentage: f64) {
        self.percentage.store(percentage);
    }

    /// 读取当前配置比例。
    pub fn GetPercentage(&self) -> f64 {
        self.percentage.load()
    }

    /// 按当前 percentage 刷新 memory limit；调整窗口内若参数未变则保留 fallback。
    pub fn UpdateMemoryLimit(&self) {
        let _guard = self.tuningLock.lock().expect("memory tuner lock poisoned");
        if global_arbitrator::UsingGlobalMemArbitration() {
            return;
        }
        // 调整进行中且服务器上限/比例未变：不覆盖临时 fallback。
        if self.adjustPercentageInProgress.load(Ordering::SeqCst)
            && self.serverMemLimitBeforeAdjust.load(Ordering::SeqCst)
                == tracker::ServerMemoryLimit.Load()
            && self.percentageBeforeAdjust.load() == self.GetPercentage()
        {
            return;
        }

        let mut memory_limit = self.calcMemoryLimit(self.GetPercentage());
        if memory_limit == i64::MAX {
            self.isValidValueSet.store(false, Ordering::SeqCst);
            memory_limit = initGOMemoryLimitValue.load(Ordering::SeqCst);
        } else {
            self.isValidValueSet.store(true, Ordering::SeqCst);
        }
        setMemoryLimit(memory_limit);
    }

    /// 计算 `ServerMemoryLimit * percentage`；禁用或结果为 0 时返回初始/MAX。
    pub fn calcMemoryLimit(&self, percentage: f64) -> i64 {
        if self.adjustDisabled.load(Ordering::SeqCst) > 0 {
            return initGOMemoryLimitValue.load(Ordering::SeqCst);
        }
        let memory_limit = (tracker::ServerMemoryLimit.Load() as f64 * percentage) as i64;
        if memory_limit == 0 {
            i64::MAX
        } else {
            memory_limit
        }
    }

    /// 是否已写入过有效 memory limit。
    pub fn isValidValueSet(&self) -> bool {
        self.isValidValueSet.load(Ordering::SeqCst)
    }

    /// 是否处于 fallback 调整窗口。
    pub fn adjustPercentageInProgress(&self) -> bool {
        self.adjustPercentageInProgress.load(Ordering::SeqCst)
    }

    /// 上次判定是否标记为“由 memory limit 触发的下次 GC”。
    pub fn nextGCTriggeredByMemoryLimit(&self) -> bool {
        self.nextGCTriggeredByMemoryLimit.load(Ordering::SeqCst)
    }
}

/// 进程级单例调谐器；注册全局内存仲裁回调以便仲裁开启时同步 limit。
pub static GlobalMemoryLimitTuner: LazyLock<Arc<MemoryLimitTuner>> = LazyLock::new(|| {
    let tuner = MemoryLimitTuner::new();
    global_arbitrator::RegisterCallbackForGlobalMemArbitrator(resetGlobalArbitratorLimit);
    tuner
});

/// 测试辅助：等待所有复位后台任务退出。
pub fn WaitMemoryLimitTunerExitInTest() {
    while MEMORY_GOROUTINE_COUNT.load(Ordering::SeqCst) > 0 {
        thread::sleep(Duration::from_millis(100));
    }
}

/// 强制初始化全局 `GlobalMemoryLimitTuner`。
pub fn init() {
    let _ = LazyLock::force(&GlobalMemoryLimitTuner);
}

/// 若启用全局内存仲裁，将 runtime limit 同步为 `ServerMemoryLimit`。
pub fn resetGlobalArbitratorLimit() {
    let _guard = GlobalMemoryLimitTuner
        .tuningLock
        .lock()
        .expect("memory tuner lock poisoned");
    if global_arbitrator::UsingGlobalMemArbitration() {
        setMemoryLimit(tracker::ServerMemoryLimit.Load() as i64);
    }
}
