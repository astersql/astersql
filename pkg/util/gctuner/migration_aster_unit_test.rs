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

// gctuner 迁移期聚合回归：finalizer、calcGCPercent、全局 Tuning、内存探测与 memory-limit 生命周期。

use serial_test::serial;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use crate::finalizer::newFinalizer;
use crate::mem::readMemoryInuse;
use crate::memory_limit_tuner::{
    MemoryLimitTuner, currentMemoryLimit, initGOMemoryLimitValue,
};
use crate::tuner::{
    EnableGOGCTuner, GetGOGC, MaxGCPercent, MinGCPercent, SetMaxGCPercent, SetMinGCPercent, Tuning,
    calcGCPercent, defaultGCPercent,
};
use task_memory::tracker::ServerMemoryLimit;

/// Finalizer 在 stop 前可重复武装；stop 后 `run` 返回 false 且不再回调。
#[test]
fn migration_finalizer_rearms_until_stopped() {
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_calls = Arc::clone(&calls);
    let finalizer = newFinalizer(Box::new(move || {
        callback_calls.fetch_add(1, Ordering::SeqCst);
    }));

    assert!(finalizer.run());
    assert!(finalizer.run());
    assert_eq!(2, calls.load(Ordering::SeqCst));

    finalizer.stop();
    assert!(!finalizer.run());
    assert!(!finalizer.run());
    assert_eq!(2, calls.load(Ordering::SeqCst));
}

/// `calcGCPercent` 在固定 GB 表上与 Go 期望一致。
#[test]
#[serial]
fn migration_calc_gc_percent_matches_go_table() {
    const GB: u64 = 1024 * 1024 * 1024;
    SetMinGCPercent(100);
    SetMaxGCPercent(500);

    assert_eq!(defaultGCPercent(), calcGCPercent(0, 0));
    assert_eq!(defaultGCPercent(), calcGCPercent(0, 1));
    assert_eq!(defaultGCPercent(), calcGCPercent(1, 0));
    assert_eq!(MaxGCPercent(), calcGCPercent(1, 3 * GB));
    assert_eq!(MaxGCPercent(), calcGCPercent(GB / 10, 4 * GB));
    assert_eq!(MaxGCPercent(), calcGCPercent(GB / 2, 4 * GB));
    assert_eq!(300, calcGCPercent(GB, 4 * GB));
    assert_eq!(166, calcGCPercent(GB + GB / 2, 4 * GB));
    assert_eq!(100, calcGCPercent(2 * GB, 4 * GB));
    assert_eq!(100, calcGCPercent(3 * GB, 4 * GB));
    assert_eq!(MinGCPercent(), calcGCPercent(4 * GB, 4 * GB));
    assert_eq!(MinGCPercent(), calcGCPercent(5 * GB, 4 * GB));
}

/// 全局 `Tuning`：有阈值时保持默认 GOGC；`Tuning(0)` 停止后仍为默认。
#[test]
#[serial]
fn migration_global_tuner_preserves_go_lifecycle() {
    SetMinGCPercent(100);
    SetMaxGCPercent(500);
    EnableGOGCTuner.store(true, Ordering::SeqCst);

    Tuning(4 * 1024 * 1024 * 1024);
    assert_eq!(defaultGCPercent(), GetGOGC());
    Tuning(8 * 1024 * 1024 * 1024);

    Tuning(0);
    assert_eq!(defaultGCPercent(), GetGOGC());
}

/// 进程有 100 MiB 活动分配时 `readMemoryInuse` 应至少报告 100 MiB。
#[test]
fn migration_memory_probe_reports_live_process_memory() {
    if crate::mem_test::run_in_isolated_process(
        "migration_aster_unit_test::migration_memory_probe_reports_live_process_memory",
    ) {
        return;
    }
    const MB: u64 = 1024 * 1024;
    let heap = vec![0x5a_u8; (100 * MB + 1) as usize];
    let inuse = readMemoryInuse();
    assert!(inuse >= 100 * MB);
    assert_eq!(0x5a, heap[heap.len() / 2]);
    std::hint::black_box(&heap);
}

/// Update / Disable / Enable / 零服务器上限路径与 Go 语义对齐。
#[test]
#[serial]
fn migration_memory_limit_update_disable_and_zero_match_go() {
    let tuner = MemoryLimitTuner::withResetInterval(Duration::from_millis(10));
    let initial = initGOMemoryLimitValue.load(Ordering::SeqCst);

    ServerMemoryLimit.Store(1 << 30);
    tuner.SetPercentage(0.8);
    tuner.UpdateMemoryLimit();
    assert_eq!((1 << 30) * 80 / 100, currentMemoryLimit());
    assert!(tuner.isValidValueSet());

    tuner.DisableAdjustMemoryLimit();
    assert_eq!(initial, currentMemoryLimit());
    assert_eq!(initial, tuner.calcMemoryLimit(0.8));

    tuner.EnableAdjustMemoryLimit();
    assert_eq!((1 << 30) * 80 / 100, currentMemoryLimit());

    ServerMemoryLimit.Store(0);
    tuner.UpdateMemoryLimit();
    assert_eq!(initial, currentMemoryLimit());
    assert!(!tuner.isValidValueSet());
}

/// 两次 `tuning` 进入调整：先 fallback 1.1，再复位到配置比例。
#[test]
#[serial]
fn migration_memory_limit_tuning_uses_two_gc_decisions_and_resets() {
    let tuner = MemoryLimitTuner::withResetInterval(Duration::from_millis(100));
    ServerMemoryLimit.Store(1024);
    tuner.SetPercentage(0.5);
    tuner.UpdateMemoryLimit();

    tuner.tuning();
    assert!(tuner.nextGCTriggeredByMemoryLimit());
    assert!(!tuner.adjustPercentageInProgress());

    tuner.tuning();
    assert!(tuner.adjustPercentageInProgress());
    let fallback_limit = tuner.calcMemoryLimit(1.1);
    let fallback_deadline = Instant::now() + Duration::from_secs(2);
    while currentMemoryLimit() != fallback_limit && Instant::now() < fallback_deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fallback_limit, currentMemoryLimit());

    let deadline = Instant::now() + Duration::from_secs(2);
    while tuner.adjustPercentageInProgress() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert!(!tuner.adjustPercentageInProgress());
    assert_eq!(
        tuner.calcMemoryLimit(tuner.GetPercentage()),
        currentMemoryLimit()
    );
}

/// issue 48741 迁移版：调整中同参 Update 保留 fallback；改服务器上限则立即重算。
#[test]
#[serial]
fn migration_memory_limit_update_during_adjustment_matches_issue_48741() {
    let tuner = MemoryLimitTuner::withResetInterval(Duration::from_millis(200));
    ServerMemoryLimit.Store(1024);
    tuner.SetPercentage(0.5);
    tuner.UpdateMemoryLimit();

    tuner.tuning();
    tuner.tuning();
    let fallback_limit = tuner.calcMemoryLimit(1.1);
    let fallback_deadline = Instant::now() + Duration::from_secs(2);
    while currentMemoryLimit() != fallback_limit && Instant::now() < fallback_deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fallback_limit, currentMemoryLimit());

    // The same values must leave the temporary fallback limit in place.
    // 参数未变：保留临时 fallback。
    tuner.UpdateMemoryLimit();
    assert_eq!(fallback_limit, currentMemoryLimit());

    // A server-limit change during adjustment must be applied immediately.
    // 调整中改服务器上限：立即按新上限*比例生效。
    ServerMemoryLimit.Store(2048);
    tuner.UpdateMemoryLimit();
    assert_eq!(1024, currentMemoryLimit());

    let deadline = Instant::now() + Duration::from_secs(2);
    while tuner.adjustPercentageInProgress() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert!(!tuner.adjustPercentageInProgress());
    assert_eq!(1024, currentMemoryLimit());
}
