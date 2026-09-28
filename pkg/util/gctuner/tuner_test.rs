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

// `tuner` 模块测试：按占用/阈值比收敛 GOGC，以及 `calcGCPercent` 查表对齐。

use crate::mem::readMemoryInuse;
use crate::tuner::{
    EnableGOGCTuner, MaxGCPercent, MinGCPercent, SetMaxGCPercent, SetMinGCPercent, calcGCPercent,
    defaultGCPercent, defaultMaxGCPercent, defaultMinGCPercent, maxGCPercent, minGCPercent,
    newTuner,
};
use serial_test::serial;

/// 最多跑 8 轮 finalizer，直到 GOGC 满足谓词。
fn run_until(tuner: &crate::tuner::Tuner, predicate: impl Fn(u32) -> bool) {
    for _ in 0..8 {
        assert!(tuner.runFinalizer());
        if predicate(tuner.getGCPercent()) {
            return;
        }
    }
    panic!("GC percent did not converge: {}", tuner.getGCPercent());
}

/// 通过改变阈值相对占用的倍数，验证 GOGC 在 max→中档→min 间收敛，并确认 Stop 后不再运行。
#[test]
#[serial]
fn test_tuner() {
    SetMinGCPercent(defaultMinGCPercent);
    SetMaxGCPercent(defaultMaxGCPercent);
    EnableGOGCTuner.store(true, std::sync::atomic::Ordering::SeqCst);

    let initial_inuse = readMemoryInuse().max(1);
    let tuner = newTuner(initial_inuse.saturating_mul(10));
    assert_eq!(initial_inuse.saturating_mul(10), tuner.getThreshold());
    assert_eq!(defaultGCPercent(), tuner.getGCPercent());

    // The Go test changes heap occupancy through 0, 1/4, 1/2, 3/4 and over
    // threshold. Rust's memory backend observes process RSS, so set thresholds
    // from each measured occupancy while preserving the same ratios.
    // Go 测试改堆占用比例；Rust 读 RSS，故以实测占用设阈值，保持相同比值。
    run_until(&tuner, |value| value == MaxGCPercent());

    let inuse = readMemoryInuse().max(1);
    tuner.setThreshold(inuse.saturating_mul(4));
    run_until(&tuner, |value| (250..=300).contains(&value));

    let inuse = readMemoryInuse().max(1);
    tuner.setThreshold(inuse.saturating_mul(2));
    run_until(&tuner, |value| (50..=100).contains(&value));

    let inuse = readMemoryInuse().max(1);
    tuner.setThreshold(inuse.saturating_mul(4) / 3);
    run_until(&tuner, |value| value == MinGCPercent());

    let inuse = readMemoryInuse().max(2);
    tuner.setThreshold(inuse - 1);
    for _ in 0..8 {
        run_until(&tuner, |value| value == MinGCPercent());
    }

    let inuse = readMemoryInuse().max(1);
    tuner.setThreshold(inuse.saturating_mul(10));
    run_until(&tuner, |value| value == MaxGCPercent());

    tuner.stop();
    assert!(!tuner.runFinalizer());
    EnableGOGCTuner.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// 用固定 GB 级占用/阈值表对齐 Go 的 `calcGCPercent` 期望值。
#[test]
#[serial]
fn test_calc_gc_percent() {
    SetMinGCPercent(defaultMinGCPercent);
    SetMaxGCPercent(defaultMaxGCPercent);
    const GB: u64 = 1024 * 1024 * 1024;

    assert_eq!(defaultGCPercent(), calcGCPercent(0, 0));
    assert_eq!(defaultGCPercent(), calcGCPercent(0, 1));
    assert_eq!(defaultGCPercent(), calcGCPercent(1, 0));
    assert_eq!(
        maxGCPercent.load(std::sync::atomic::Ordering::SeqCst),
        calcGCPercent(1, 3 * GB)
    );
    assert_eq!(
        maxGCPercent.load(std::sync::atomic::Ordering::SeqCst),
        calcGCPercent(GB / 10, 4 * GB)
    );
    assert_eq!(
        maxGCPercent.load(std::sync::atomic::Ordering::SeqCst),
        calcGCPercent(GB / 2, 4 * GB)
    );
    assert_eq!(300, calcGCPercent(GB, 4 * GB));
    assert_eq!(166, calcGCPercent(GB * 3 / 2, 4 * GB));
    assert_eq!(100, calcGCPercent(2 * GB, 4 * GB));
    assert_eq!(100, calcGCPercent(3 * GB, 4 * GB));
    assert_eq!(
        minGCPercent.load(std::sync::atomic::Ordering::SeqCst),
        calcGCPercent(4 * GB, 4 * GB)
    );
    assert_eq!(
        minGCPercent.load(std::sync::atomic::Ordering::SeqCst),
        calcGCPercent(5 * GB, 4 * GB)
    );
}

/// Go checks the minimum before the maximum, so inverted public bounds return
/// the configured minimum instead of panicking.
#[test]
#[serial]
fn calc_gc_percent_preserves_go_order_for_inverted_bounds() {
    SetMinGCPercent(400);
    SetMaxGCPercent(200);

    assert_eq!(400, calcGCPercent(1, 4));

    SetMinGCPercent(defaultMinGCPercent);
    SetMaxGCPercent(defaultMaxGCPercent);
}
