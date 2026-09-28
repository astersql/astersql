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

// `MemoryLimitTuner` 单元测试：两阶段调整、fallback 复位、禁用/启用与 issue 48741。

use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use crate::memory_limit_tuner::{
    MemoryLimitTuner, WaitMemoryLimitTunerExitInTest, currentMemoryLimit, fallbackPercentage,
    initGOMemoryLimitValue,
};
use serial_test::serial;
use task_memory::tracker::{MemoryLimitGCTotal, ServerMemoryLimit, TriggerMemoryLimitGC};

/// 在超时内轮询直到条件为真，否则断言失败。
fn eventually(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "condition did not become true in {timeout:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

/// 触发两轮 finalizer，进入调整窗口（与 Go 两阶段 finalizer 流一致）。
fn start_adjustment(tuner: &MemoryLimitTuner) {
    // The first collection arms the next memory-limit collection; the second
    // enters the adjustment window, matching the two-stage Go finalizer flow.
    // 第一次武装 next-GC 标志；第二次进入调整窗口。
    assert!(tuner.runFinalizer());
    assert!(tuner.nextGCTriggeredByMemoryLimit());
    assert!(tuner.runFinalizer());
    eventually(Duration::from_secs(1), || {
        tuner.adjustPercentageInProgress()
    });
}

/// Go 在首次判定下一次 GC 将由 memory limit 触发时就发布全局触发标志。
#[test]
#[serial]
fn first_memory_limit_gc_decision_sets_trigger_flag() {
    initGOMemoryLimitValue.store(i64::MAX, Ordering::SeqCst);
    TriggerMemoryLimitGC.Store(false);

    let tuner = MemoryLimitTuner::withResetInterval(Duration::from_millis(10));
    ServerMemoryLimit.Store(1);
    tuner.SetPercentage(1.0);
    tuner.UpdateMemoryLimit();

    assert!(tuner.runFinalizer());
    assert!(tuner.nextGCTriggeredByMemoryLimit());
    assert!(TriggerMemoryLimitGC.Load());
    assert!(!tuner.adjustPercentageInProgress());
    tuner.Stop();
    TriggerMemoryLimitGC.Store(false);
}

/// 完整生命周期：设 80% limit → 进入 fallback 110% → 复位 → Stop。
#[test]
#[serial]
fn test_global_memory_tuner() {
    initGOMemoryLimitValue.store(i64::MAX, Ordering::SeqCst);

    let tuner = MemoryLimitTuner::withResetInterval(Duration::from_millis(200));
    ServerMemoryLimit.Store(1 << 20);
    tuner.SetPercentage(0.8);
    tuner.UpdateMemoryLimit();
    assert!(tuner.isValidValueSet());
    assert_eq!(tuner.calcMemoryLimit(0.8), currentMemoryLimit());

    let gc_total = MemoryLimitGCTotal.Load();
    start_adjustment(&tuner);
    eventually(Duration::from_secs(1), || {
        MemoryLimitGCTotal.Load() > gc_total
    });
    assert_eq!(
        tuner.calcMemoryLimit(fallbackPercentage),
        currentMemoryLimit()
    );

    eventually(Duration::from_secs(1), || {
        !tuner.adjustPercentageInProgress()
    });
    assert_eq!(
        tuner.calcMemoryLimit(tuner.GetPercentage()),
        currentMemoryLimit()
    );
    tuner.Stop();
    assert!(!tuner.runFinalizer());
}

/// issue 48741：调整中 UpdateMemoryLimit 在参数未变时保留 fallback；服务器上限变更则立即重算。
#[test]
#[serial]
fn test_issue48741() {
    initGOMemoryLimitValue.store(i64::MAX, Ordering::SeqCst);

    // Unchanged server limit: UpdateMemoryLimit must retain the 110% fallback
    // during adjustment and reset to 80% when the worker finishes.
    // 服务器上限未变：调整期间 Update 应保留 110% fallback，结束后回到 80%。
    let unchanged = MemoryLimitTuner::withResetInterval(Duration::from_millis(200));
    ServerMemoryLimit.Store(1 << 20);
    unchanged.SetPercentage(0.8);
    unchanged.UpdateMemoryLimit();
    let before = MemoryLimitGCTotal.Load();
    start_adjustment(&unchanged);
    eventually(Duration::from_secs(1), || {
        MemoryLimitGCTotal.Load() > before
    });
    unchanged.UpdateMemoryLimit();
    assert_eq!(
        unchanged.calcMemoryLimit(fallbackPercentage),
        currentMemoryLimit()
    );
    eventually(Duration::from_secs(1), || {
        !unchanged.adjustPercentageInProgress()
    });
    assert_eq!(unchanged.calcMemoryLimit(0.8), currentMemoryLimit());

    // With the next-GC flag still armed, collection immediately starts another
    // adjustment after the limit returns to 80%, as in the Go regression case.
    // next-GC 仍武装时，复位后下一轮收集会立刻再开调整。
    let after_first_adjustment = MemoryLimitGCTotal.Load();
    assert!(unchanged.runFinalizer());
    eventually(Duration::from_secs(1), || {
        MemoryLimitGCTotal.Load() > after_first_adjustment
    });
    eventually(Duration::from_secs(1), || {
        !unchanged.adjustPercentageInProgress()
    });
    unchanged.Stop();

    // Changed server limit: an update during adjustment must apply the new
    // 1.5 MiB * 80% limit instead of retaining the old fallback value.
    // 调整中改服务器上限：应立即应用新的 1.5MiB*80%，而非旧 fallback。
    let changed = MemoryLimitTuner::withResetInterval(Duration::from_millis(200));
    ServerMemoryLimit.Store(1 << 20);
    changed.SetPercentage(0.8);
    changed.UpdateMemoryLimit();
    start_adjustment(&changed);
    ServerMemoryLimit.Store(1500 << 10);
    changed.UpdateMemoryLimit();
    assert!(changed.adjustPercentageInProgress());
    assert_eq!((1500_i64 << 10) * 80 / 100, currentMemoryLimit());
    eventually(Duration::from_secs(1), || {
        !changed.adjustPercentageInProgress()
    });
    assert_eq!((1500_i64 << 10) * 80 / 100, currentMemoryLimit());
    changed.Stop();
}

/// 禁用调整时 limit 回退到初始值；重新启用后按 percentage 计算。
#[test]
#[serial]
fn test_set_memory_limit() {
    initGOMemoryLimitValue.store(i64::MAX, Ordering::SeqCst);
    let tuner = MemoryLimitTuner::withResetInterval(Duration::from_millis(10));
    ServerMemoryLimit.Store(1 << 30);
    tuner.SetPercentage(0.8);

    tuner.DisableAdjustMemoryLimit();
    tuner.UpdateMemoryLimit();
    assert_eq!(
        initGOMemoryLimitValue.load(Ordering::SeqCst),
        currentMemoryLimit()
    );

    tuner.EnableAdjustMemoryLimit();
    tuner.UpdateMemoryLimit();
    assert_eq!((1_i64 << 30) * 80 / 100, currentMemoryLimit());
    tuner.Stop();
}

/// `Start` 必须接入生产运行时驱动；不手动调用 `runFinalizer` 也应完成两阶段判定。
#[test]
#[serial]
fn memory_limit_tuner_runs_without_manual_finalizer() {
    initGOMemoryLimitValue.store(i64::MAX, Ordering::SeqCst);
    let tuner = MemoryLimitTuner::withResetInterval(Duration::from_millis(10));
    tuner.Stop();
    tuner.Start();
    ServerMemoryLimit.Store(1);
    tuner.SetPercentage(1.0);
    tuner.UpdateMemoryLimit();

    let before = MemoryLimitGCTotal.Load();
    eventually(Duration::from_secs(2), || {
        MemoryLimitGCTotal.Load() > before
    });
    tuner.Stop();
    WaitMemoryLimitTunerExitInTest();
}
