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

// MemArbitrator 主路径与工具函数单元测试。
//
// 对应 Go TestMemArbitrator* / TestBasicUtils / TestBench：覆盖模式切换、
// 配额分配、软限制、风险阈值、ArbitrationContext，以及 ConcurrentBudget 并发与 wrapList 基准行为。

#![allow(non_snake_case)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use crate::*;

// 下列测试名与 Go 测试的对应关系：
// Go mapping:
//   TestMemArbitratorSwitchMode -> test_mem_arbitrator_switch_mode
//   TestMemArbitrator           -> test_mem_arbitrator
//   TestBasicUtils              -> test_basic_utils
//   TestBench                   -> test_bench
//   BenchmarkWrapList           -> benchmark_wrap_list_behavior
//   BenchmarkList               -> benchmark_list_behavior

/// 测试用 ArbitrateHelper：统计 Stop/Finish 调用次数。
struct Helper {
    stopped: AtomicUsize,
    finished: AtomicUsize,
}

impl Helper {
    /// 构造计数为 0 的 Helper。
    fn new() -> Self {
        Self {
            stopped: AtomicUsize::new(0),
            finished: AtomicUsize::new(0),
        }
    }
}

impl ArbitrateHelper for Helper {
    fn Stop(&self, _reason: ArbitratorStopReason) -> bool {
        self.stopped.fetch_add(1, Ordering::SeqCst);
        true
    }

    fn HeapInuse(&self) -> i64 {
        0
    }

    fn Finish(&self) {
        self.finished.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
/// 工作模式切换、非法模式拒绝与任务分桶计数。
fn test_mem_arbitrator_switch_mode() {
    let mut m = NewMemArbitrator(1_000);
    assert_eq!(m.WorkMode(), ArbitratorModeDisable);
    assert!(!m.SetWorkMode(ArbitratorModeDisable));
    assert!(!m.SetWorkMode(ArbitratorWorkMode(99)));

    assert!(m.SetWorkMode(ArbitratorModeStandard));
    assert_eq!(m.WorkMode(), ArbitratorModeStandard);
    assert!(!m.SetWorkMode(ArbitratorModeStandard));

    m.record_task(ArbitrationPriorityMedium, false);
    assert_eq!(m.TaskNumByPattern(), [0, 1, 0, 0]);

    assert!(m.SetWorkMode(ArbitratorModePriority));
    m.record_task(ArbitrationPriorityLow, true);
    m.record_task(ArbitrationPriorityHigh, false);
    assert_eq!(m.TaskNumByPattern(), [1, 1, 1, 1]);

    assert!(m.SetWorkMode(ArbitratorModeDisable));
    assert_eq!(m.WorkMode(), ArbitratorModeDisable);
}

#[test]
/// 端到端：默认 Limit、allocate/release、软限制、堆风险、Context.stop、memHangRisk。
fn test_mem_arbitrator() {
    let default_arbitrator = NewMemArbitrator(-1);
    assert_eq!(default_arbitrator.Limit(), DefMaxLimit);

    let mut m = NewMemArbitrator(1_000);
    assert_eq!(m.Allocated(), 0);
    assert_eq!(m.OutOfControl(), 0);
    assert_eq!(m.available(), 1_000);

    assert!(m.allocate(600));
    assert_eq!(m.Allocated(), 600);
    assert_eq!(m.quotaAvailable(), 400);
    assert_eq!(m.available(), 400);
    assert!(!m.allocate(401));
    assert_eq!(m.Allocated(), 600);
    assert!(m.allocate(400));
    assert_eq!(m.Allocated(), 1_000);
    m.release(250);
    assert_eq!(m.Allocated(), 750);
    m.release(i64::MAX);
    assert_eq!(m.Allocated(), 0);

    assert!(!m.SetLimit(1_000));
    assert!(!m.SetLimit(0));
    assert!(m.SetLimit(2_000));
    assert_eq!(m.memRisk(), 1_800);
    assert_eq!(m.oomRisk(), 1_900);

    m.SetSoftLimit(1_200, 0.0, SoftLimitModeSpecified);
    assert_eq!(m.SoftLimit(), 1_200);
    m.SetSoftLimit(0, 0.75, SoftLimitModeSpecified);
    assert_eq!(m.SoftLimit(), 1_500);
    m.SetSoftLimit(0, 0.0, SoftLimitModeDisable);
    assert_eq!(m.SoftLimit(), 1_900);

    m.set_runtime_heap(1_799);
    assert!(!m.AtMemRisk());
    assert!(!m.AtOOMRisk());
    m.set_runtime_heap(1_800);
    assert!(m.AtMemRisk());
    assert!(!m.AtOOMRisk());
    m.set_runtime_heap(1_900);
    assert!(m.AtMemRisk());
    assert!(m.AtOOMRisk());

    let helper = Arc::new(Helper::new());
    let ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(helper.clone()),
        ArbitrationPriorityHigh,
        true,
        true,
    );
    assert!(ctx.available());
    assert_eq!(ctx.memPriority, ArbitrationPriorityHigh);
    assert!(ctx.waitAverse);
    assert!(ctx.preferPrivilege);
    ctx.stop(ArbitratorOOMRiskKill);
    ctx.stop(ArbitratorStandardCancel);
    assert!(!ctx.available());
    assert_eq!(helper.stopped.load(Ordering::SeqCst), 1);

    m.setUnixTimeSec(1_725_000_123);
    assert_eq!(m.approxUnixTimeSec(), 1_725_000_123);
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
    assert!(!memHangRisk(
        100,
        100,
        start + Duration::from_secs(5),
        start
    ));
    assert!(memHangRisk(99, 100, start + Duration::from_secs(1), start));
    assert!(memHangRisk(100, 100, start + Duration::from_secs(6), start));
}

/// 内嵌 utils.rs 的哈希/分片/通知器/wrapList/比例换算等 Go 对齐用例。
mod basic_utils_fixture {
    const prime64: u64 = 1_099_511_628_211;
    const initHashKey: u64 = 14_695_981_039_346_656_037;
    const baseQuotaUnit: i64 = 4 * byteSizeKB;

    include!("utils.rs");

    /// 跑一遍 utils 与枚举字符串的黄金断言。
    pub fn verify_go_cases() {
        const COUNT: u64 = 1 << 8;
        let bg_id = 4_068_484_684_u64;
        let odd = (0..COUNT)
            .filter(|i| shardIndexByUID(bg_id + i * 2, COUNT - 1) & 1 != 0)
            .count();
        assert_eq!(odd, COUNT as usize / 2);

        assert_eq!(getQuotaShard(0, 17), 0);
        assert_eq!(getQuotaShard(baseQuotaUnit - 1, 17), 0);
        assert_eq!(getQuotaShard(baseQuotaUnit, 17), 1);
        assert_eq!(getQuotaShard(baseQuotaUnit * 2 - 1, 17), 1);
        assert_eq!(getQuotaShard(baseQuotaUnit * 2, 17), 2);
        assert_eq!(getQuotaShard(i64::MAX, 17), 16);

        let notifier = NewNotifer();
        assert!(!notifier.isAwake());
        notifier.Wake();
        assert!(notifier.isAwake());
        notifier.Wake();
        notifier.WeakWake();
        notifier.Wait();
        assert!(!notifier.isAwake());

        let mut data = wrapList::new();
        data.init();
        assert!(data.empty());
        assert_eq!(data.front(), None);
        let first = data.pushBack(1_i64);
        let second = data.pushBack(2_i64);
        assert_eq!(data.front(), Some(&1));
        data.remove(second);
        data.pushBack(3);
        assert_eq!(data.popFront(), Some(1));
        let first_again = data.pushBack(1);
        data.moveToFront(first_again);
        assert_eq!(data.front(), Some(&1));
        assert_eq!(data.size(), 2);
        assert!(first.valid());

        assert_eq!(nextPow2(0), 1);
        for n in 1..63 {
            let x = 1_u64 << n;
            assert_eq!(nextPow2(x), x);
            if x > 2 {
                assert_eq!(nextPow2(x - 1), x);
            }
        }

        let millis = nowUnixMilli();
        let seconds = nowUnixSec();
        assert!(millis / 1_000 >= seconds - 1);
        assert_eq!(calcRatio(3, 2), 1_500);
        assert_eq!(multiRatio(10, 500), 5);
        assert_eq!(intoRatio(0.75), 750);
    }

    /// 交替 push/pop 锻炼 wrapList，返回最终 size。
    pub fn exercise_wrap_list(iterations: usize) -> i64 {
        let mut data = wrapList::new();
        let mut pushes = 0_i64;
        let mut pops = 0_i64;
        for i in 0..iterations {
            if i % 200 >= 100 {
                if data.popFront().is_some() {
                    pops += 1;
                }
            } else {
                data.pushBack(1_234_i64);
                pushes += 1;
            }
        }
        assert_eq!(data.size(), pushes - pops);
        data.size()
    }
}

#[test]
/// 调用 fixture 并断言优先级/模式/停止原因字符串。
fn test_basic_utils() {
    basic_utils_fixture::verify_go_cases();
    assert_eq!(ArbitrationPriorityLow.String(), "LOW");
    assert_eq!(ArbitrationPriorityMedium.String(), "MEDIUM");
    assert_eq!(ArbitrationPriorityHigh.String(), "HIGH");
    assert_eq!(ArbitratorModeStandard.String(), "standard");
    assert_eq!(ArbitratorModePriority.String(), "priority");
    assert_eq!(ArbitratorModeDisable.String(), "disable");
    assert_eq!(ArbitratorOOMRiskKill.String(), "KILL(out-of-memory)");
    assert_eq!(ArbitratorStopReason(99).String(), "UNKNOWN");
}

#[test]
/// 多线程 ConcurrentBudget 耗尽计数，以及大批量 record_task 分桶。
fn test_bench() {
    const N: usize = 3_000;
    const WORKERS: usize = 8;
    let mut workers = Vec::new();
    for worker in 0..WORKERS {
        workers.push(thread::spawn(move || {
            let mut exhausted = 0;
            for i in (worker..N).step_by(WORKERS) {
                let budget = ConcurrentBudget::new(150);
                assert!(budget.ConsumeQuota(i as i64, 100).is_ok());
                if budget.ConsumeQuota(i as i64 + 1, 51).is_err() {
                    exhausted += 1;
                }
                assert_eq!(budget.used(), 151);
                assert!(budget.ConsumeQuota(i as i64 + 2, -51).is_ok());
                assert_eq!(budget.used(), 100);
            }
            exhausted
        }));
    }
    let exhausted: usize = workers.into_iter().map(|w| w.join().unwrap()).sum();
    assert_eq!(exhausted, N);

    let mut m = NewMemArbitrator(4 * 1_073_741_824);
    assert!(m.SetWorkMode(ArbitratorModePriority));
    for i in 0..N {
        let priority = match i % 6 {
            0 | 1 => ArbitrationPriorityLow,
            2 | 3 => ArbitrationPriorityMedium,
            _ => ArbitrationPriorityHigh,
        };
        m.record_task(priority, i % 2 == 1);
    }
    assert_eq!(m.TaskNumByPattern(), [1_000, 1_000, 1_000, 1_500]);
}

#[test]
/// Go `ConcurrentBudget.Reserve` treats its argument as the target capacity,
/// never as an increment, and never shrinks below current usage.
fn concurrent_budget_reserve_uses_absolute_capacity() {
    let budget = ConcurrentBudget::new(10);

    budget.Reserve(20);
    assert_eq!(budget.capacity(), 20);

    budget.Reserve(15);
    assert_eq!(budget.capacity(), 20);

    assert!(budget.ConsumeQuota(1, 25).is_err());
    budget.Reserve(22);
    assert_eq!(budget.capacity(), 25);
}

#[test]
/// Digest maxima cover the current and previous 60-second buckets; an older
/// peak must age out once the second newer bucket is observed.
fn digest_profile_maximum_expires_with_go_window() {
    let m = NewMemArbitrator(10_000);

    m.UpdateDigestProfileCache(7, 1_009, 0);
    m.UpdateDigestProfileCache(7, 107, 60);
    assert_eq!(m.GetDigestProfileCache(7, 61), Some(1_009));

    m.UpdateDigestProfileCache(7, 107, 120);
    assert_eq!(m.GetDigestProfileCache(7, 121), Some(107));
}

#[test]
/// wrapList 微基准冒烟。
fn benchmark_wrap_list_behavior() {
    assert_eq!(basic_utils_fixture::exercise_wrap_list(20_000), 0);
}

#[test]
/// 对照用的标准 VecDeque 交替 push/pop 行为。
fn benchmark_list_behavior() {
    let mut data = VecDeque::new();
    let mut pushes = 0_i64;
    let mut pops = 0_i64;
    for i in 0..20_000 {
        if i % 200 >= 100 {
            if data.pop_front().is_some() {
                pops += 1;
            }
        } else {
            data.push_back(1_234_i64);
            pushes += 1;
        }
    }
    assert_eq!(data.len() as i64, pushes - pops);
}
