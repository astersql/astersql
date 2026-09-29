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

#[test]
fn go_merge_26_unavailable_context_rejects_allocation_in_both_modes() {
    for mode in [ArbitratorModeStandard, ArbitratorModePriority] {
        for unavailable in 0..3 {
            let m = NewMemArbitrator(100);
            assert!(m.SetWorkMode(mode));
            let root = m.EmplaceRootPool(1).unwrap();
            let context = match unavailable {
                0 => None,
                1 => Some(NewArbitrationContext(
                    CancelReceiver::none(),
                    None,
                    ArbitrationPriorityMedium,
                    true,
                    false,
                )),
                _ => {
                    let context = NewArbitrationContext(
                        CancelReceiver::none(),
                        Some(Arc::new(Helper::new())),
                        ArbitrationPriorityMedium,
                        true,
                        false,
                    );
                    context.stop(ArbitratorOOMRiskKill);
                    Some(context)
                }
            };
            assert!(m.RestartEntryByContext(root, context));
            assert_eq!(m.RequestQuota(root, 200), ArbitrateFail);
            assert_eq!(m.TaskNum(), 0);
        }
    }
}

#[test]
fn go_merge_26_soft_risk_reaches_oom_without_running_pool() {
    let m = NewMemArbitrator(1_000);
    let stats = ArbitratorRuntimeStats {
        heap_alloc: 900,
        heap_inuse: 960,
        ..Default::default()
    };
    m.HandleRuntimeStats(stats);
    assert!(m.AtMemRisk());
    assert!(!m.AtOOMRisk());
    thread::sleep(Duration::from_millis(1_050));
    m.HandleRuntimeStats(stats);
    assert!(m.AtOOMRisk());
    assert_eq!(m.ExecMetrics().Risk.OOM, 1);
    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_inuse: 1_001,
        ..stats
    });
    assert_eq!(m.ExecMetrics().Risk.OOM, 1);
}

#[test]
fn go_merge_26_restored_runtime_tuning_updates_magnif_and_pool_cap() {
    let m = NewMemArbitrator(1_000);
    m.RestoreRuntimeMemState(1_300, 4_096);
    assert_eq!(m.MemMagnif(), 1_300);
    assert_eq!(m.SuggestPoolInitCap(), 4_096);
}

#[test]
fn go_merge_26_oom_reclaims_zero_quota_context_heap() {
    struct UsedHelper(AtomicUsize);
    impl ArbitrateHelper for UsedHelper {
        fn Stop(&self, _: ArbitratorStopReason) -> bool {
            self.0.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn HeapInuse(&self) -> i64 {
            10_000
        }
        fn Finish(&self) {}
    }
    let m = NewMemArbitrator(1_000);
    let root = m.EmplaceRootPool(1).unwrap();
    let helper = Arc::new(UsedHelper(AtomicUsize::new(0)));
    let context = NewArbitrationContext(
        CancelReceiver::none(),
        Some(helper.clone()),
        ArbitrationPriorityMedium,
        false,
        true,
    );
    assert!(m.RestartEntryByContext(root, context));
    assert_eq!(m.Allocated(), 0);
    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 1_100,
        heap_inuse: 1_100,
        ..Default::default()
    });
    assert_eq!(helper.0.load(Ordering::SeqCst), 1);
    assert_eq!(m.RequestQuota(root, 1), ArbitrateFail);
    assert_eq!(
        m.ExecMetrics().Risk.OOMKill[ArbitrationPriorityMedium.0 as usize],
        1
    );
}

#[test]
fn go_merge_26_digest_id_builder_distinguishes_boundaries_and_order() {
    fn digest(parts: &[&str]) -> u64 {
        let mut builder = NewDigestIDBuilder();
        for part in parts {
            builder.AddString(part);
        }
        builder.Sum64()
    }
    assert_ne!(digest(&["ab", "c"]), digest(&["a", "bc"]));
    assert_ne!(digest(&[]), digest(&[""]));
    assert_ne!(
        digest(&["db1", "t1", "db2", "t2"]),
        digest(&["db2", "t2", "db1", "t1"])
    );
}

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
/// Digest maxima cover the current and previous 30-second buckets; an older
/// peak must age out once the second newer bucket is observed.
fn digest_profile_maximum_expires_with_go_window() {
    let m = NewMemArbitrator(10_000);

    m.UpdateDigestProfileCache(7, 1_009, 0);
    m.UpdateDigestProfileCache(7, 107, 30);
    assert_eq!(m.GetDigestProfileCache(7, 31), Some(1_009));

    m.UpdateDigestProfileCache(7, 107, 60);
    assert_eq!(m.GetDigestProfileCache(7, 61), Some(107));
}

#[test]
fn go_merge_24_digest_profile_skips_invalid_id_and_uses_30_second_windows() {
    let m = NewMemArbitrator(10_000);
    m.UpdateDigestProfileCache(0, 9_999, 30);
    assert_eq!(m.GetDigestProfileCache(0, 30), None);

    m.UpdateDigestProfileCache(7, 1_009, 30);
    m.UpdateDigestProfileCache(7, 107, 60);
    assert_eq!(m.GetDigestProfileCache(7, 61), Some(1_009));
    m.UpdateDigestProfileCache(7, 107, 90);
    assert_eq!(m.GetDigestProfileCache(7, 91), Some(107));
}

#[test]
fn go_merge_24_oom_reclaims_multiple_zero_quota_contexts() {
    struct UsedHelper {
        stopped: AtomicUsize,
    }
    impl ArbitrateHelper for UsedHelper {
        fn Stop(&self, _: ArbitratorStopReason) -> bool {
            self.stopped.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn HeapInuse(&self) -> i64 {
            100
        }
        fn Finish(&self) {}
    }
    let m = NewMemArbitrator(1_000);
    let helpers: Vec<_> = (0..3)
        .map(|_| {
            Arc::new(UsedHelper {
                stopped: AtomicUsize::new(0),
            })
        })
        .collect();
    for (uid, helper) in helpers.iter().enumerate() {
        let root = m.EmplaceRootPool(uid as u64).unwrap();
        let ctx = NewArbitrationContext(
            CancelReceiver::none(),
            Some(helper.clone()),
            ArbitrationPriorityLow,
            false,
            false,
        );
        assert!(m.RestartEntryByContext(root, ctx));
    }
    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 1_100,
        heap_inuse: 1_100,
        ..Default::default()
    });
    assert_eq!(
        helpers
            .iter()
            .map(|helper| helper.stopped.load(Ordering::SeqCst))
            .sum::<usize>(),
        2
    );
    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 1_100,
        heap_inuse: 1_100,
        ..Default::default()
    });
    assert_eq!(
        helpers
            .iter()
            .map(|helper| helper.stopped.load(Ordering::SeqCst))
            .sum::<usize>(),
        2
    );
}

#[test]
fn go_merge_24_reset_clears_context_without_finishing_helper() {
    let m = NewMemArbitrator(1_000);
    let helper = Arc::new(Helper::new());
    let root = m.EmplaceRootPool(1).unwrap();
    let context = NewArbitrationContext(
        CancelReceiver::none(),
        Some(helper.clone()),
        ArbitrationPriorityMedium,
        false,
        false,
    );
    assert!(m.RestartEntryByContext(root, context));
    assert!(m.ResetRootPoolByID(1, 0, false));
    assert_eq!(helper.finished.load(Ordering::SeqCst), 0);
}

#[test]
fn go_merge_24_helper_reports_both_root_and_heap_usage() {
    let helper = Helper::new();
    let usage = helper.MemUsage();
    assert_eq!(usage.RootPoolUsed, 0);
    assert_eq!(usage.HeapInuse, 0);
}

#[test]
fn go_merge_24_oom_prefers_larger_quota_before_heap_usage() {
    struct UsedHelper {
        used: i64,
        stopped: AtomicUsize,
    }
    impl ArbitrateHelper for UsedHelper {
        fn Stop(&self, _: ArbitratorStopReason) -> bool {
            self.stopped.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn HeapInuse(&self) -> i64 {
            self.used
        }
        fn Finish(&self) {}
    }
    let m = NewMemArbitrator(1_000);
    let larger_quota = Arc::new(UsedHelper {
        used: 200,
        stopped: AtomicUsize::new(0),
    });
    let larger_heap = Arc::new(UsedHelper {
        used: 300,
        stopped: AtomicUsize::new(0),
    });
    for (uid, quota, helper) in [
        (1, 300, larger_quota.clone()),
        (2, 100, larger_heap.clone()),
    ] {
        let root = m.EmplaceRootPool(uid).unwrap();
        let ctx = NewArbitrationContext(
            CancelReceiver::none(),
            Some(helper),
            ArbitrationPriorityLow,
            false,
            false,
        );
        assert!(m.RestartEntryByContext(root, ctx));
        assert_eq!(m.RequestQuota(root, quota), ArbitrateOk);
    }
    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 1_100,
        heap_inuse: 1_100,
        ..Default::default()
    });
    assert_eq!(larger_quota.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(larger_heap.stopped.load(Ordering::SeqCst), 0);
}

#[test]
fn go_merge_24_soft_risk_waits_before_killing_sql() {
    struct UsedHelper(AtomicUsize);
    impl ArbitrateHelper for UsedHelper {
        fn Stop(&self, _: ArbitratorStopReason) -> bool {
            self.0.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn HeapInuse(&self) -> i64 {
            100
        }
        fn Finish(&self) {}
    }
    let m = NewMemArbitrator(1_000);
    let helper = Arc::new(UsedHelper(AtomicUsize::new(0)));
    let root = m.EmplaceRootPool(1).unwrap();
    let ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(helper.clone()),
        ArbitrationPriorityLow,
        false,
        false,
    );
    assert!(m.RestartEntryByContext(root, ctx));
    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 960,
        heap_inuse: 960,
        ..Default::default()
    });
    assert!(m.AtMemRisk());
    assert_eq!(helper.0.load(Ordering::SeqCst), 0);
    thread::sleep(Duration::from_millis(1_010));
    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 960,
        heap_inuse: 960,
        ..Default::default()
    });
    assert_eq!(helper.0.load(Ordering::SeqCst), 1);
}

#[test]
fn go_merge_24_emplace_reports_whether_root_was_created() {
    let m = NewMemArbitrator(1_000);
    let (created, first) = m.EmplaceRootPoolWithStatus(7).unwrap();
    assert!(created);
    let (created, second) = m.EmplaceRootPoolWithStatus(7).unwrap();
    assert!(!created);
    assert_eq!(first, second);
    assert_eq!(m.RootPoolNum(), 1);
}

#[test]
fn go_merge_24_reset_does_not_inflate_priority_buffer() {
    let m = NewMemArbitrator(1_000);
    m.SetWorkMode(ArbitratorModePriority);
    let root = m.EmplaceRootPool(1).unwrap();
    let ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(Arc::new(Helper::new())),
        ArbitrationPriorityLow,
        false,
        false,
    );
    assert!(m.RestartEntryByContext(root, ctx));
    assert!(m.ResetRootPoolByID(1, 900, true));
    assert_eq!(m.available(), 1_000);
}

#[test]
fn go_merge_24_priority_buffer_tracks_live_heap_and_expires() {
    struct UsedHelper;
    impl ArbitrateHelper for UsedHelper {
        fn Stop(&self, _: ArbitratorStopReason) -> bool {
            true
        }
        fn HeapInuse(&self) -> i64 {
            100
        }
        fn Finish(&self) {}
    }
    let m = NewMemArbitrator(1_000);
    m.SetWorkMode(ArbitratorModePriority);
    let root = m.EmplaceRootPool(1).unwrap();
    let ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(Arc::new(UsedHelper)),
        ArbitrationPriorityLow,
        false,
        false,
    );
    assert!(m.RestartEntryByContext(root, ctx));
    m.setUnixTimeSec(30);
    m.RunOneRound();
    assert_eq!(m.available(), 900);
    assert!(m.ResetRootPoolByID(1, 0, false));
    m.setUnixTimeSec(60);
    m.RunOneRound();
    assert_eq!(m.available(), 900);
    m.setUnixTimeSec(90);
    m.RunOneRound();
    assert_eq!(m.available(), 1_000);
    m.SetWorkMode(ArbitratorModeStandard);
    assert_eq!(m.available(), 1_000);
}

#[test]
fn go_merge_24_await_free_rejects_growth_at_oom_risk() {
    let m = NewMemArbitrator(1_000);
    m.set_runtime_heap(950);
    assert!(m.AtOOMRisk());
    assert!(!m.ConsumeQuotaFromAwaitFreePool(1, 1));
    assert_eq!(m.ExecMetrics().AwaitFree.Fail, 1);

    let off_heap = NewMemArbitrator(1_000);
    off_heap.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 800,
        heap_inuse: 800,
        mem_off_heap: 149,
        ..Default::default()
    });
    assert!(!off_heap.AtOOMRisk());
    assert!(!off_heap.ConsumeQuotaFromAwaitFreePool(2, 1));
}

#[test]
fn go_merge_24_pool_medium_uses_recent_consumption_median() {
    let m = NewMemArbitrator(10_000);
    m.setUnixTimeSec(30);
    for (uid, consumed) in [(1, 100), (2, 200)] {
        let root = m.EmplaceRootPool(uid).unwrap();
        let ctx = NewArbitrationContext(
            CancelReceiver::none(),
            Some(Arc::new(Helper::new())),
            ArbitrationPriorityLow,
            false,
            false,
        );
        assert!(m.RestartEntryByContext(root, ctx));
        assert!(m.ResetRootPoolByID(uid, consumed, true));
    }
    m.RunOneRound();
    assert_eq!(m.SuggestPoolInitCap(), 120);
}

#[test]
fn go_merge_24_safe_inuse_does_not_start_mem_risk_from_heap_alloc_alone() {
    let m = NewMemArbitrator(1_000);
    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 960,
        heap_inuse: 800,
        ..Default::default()
    });
    assert!(!m.AtMemRisk());
    assert!(!m.AtOOMRisk());
}

#[test]
fn go_merge_24_off_heap_counts_toward_hard_oom_risk() {
    let m = NewMemArbitrator(1_000);
    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 800,
        heap_inuse: 900,
        mem_off_heap: 200,
        ..Default::default()
    });
    assert!(m.AtMemRisk());
    assert!(m.AtOOMRisk());
}

#[test]
fn go_merge_24_context_cache_expires_idle_entries_without_removing_roots() {
    let m = NewMemArbitrator(1_000);
    m.setUnixTimeSec(30);
    let root = m.EmplaceRootPool(1).unwrap();
    let ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(Arc::new(Helper::new())),
        ArbitrationPriorityLow,
        false,
        false,
    );
    assert!(m.RestartEntryByContext(root, ctx));
    assert_eq!(m.ContextCacheNumForTest(), 1);
    assert!(m.ResetRootPoolByID(1, 0, false));
    m.setUnixTimeSec(631);
    m.RunOneRound();
    assert_eq!(m.ContextCacheNumForTest(), 0);
    assert!(m.FindRootPool(1).is_some());
}

#[test]
fn go_merge_24_priority_reclaim_stops_after_enough_quota() {
    let m = Arc::new(NewMemArbitrator(1_000));
    m.SetWorkMode(ArbitratorModePriority);
    let larger = Arc::new(Helper::new());
    let smaller = Arc::new(Helper::new());
    for (uid, quota, helper) in [(1, 400, larger.clone()), (2, 300, smaller.clone())] {
        let root = m.EmplaceRootPool(uid).unwrap();
        let ctx = NewArbitrationContext(
            CancelReceiver::none(),
            Some(helper),
            ArbitrationPriorityLow,
            false,
            false,
        );
        assert!(m.RestartEntryByContext(root, ctx));
        assert_eq!(m.RequestQuota(root, quota), ArbitrateOk);
    }
    let high = m.EmplaceRootPool(3).unwrap();
    let ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(Arc::new(Helper::new())),
        ArbitrationPriorityHigh,
        false,
        false,
    );
    assert!(m.RestartEntryByContext(high, ctx));
    let (tx, rx) = std::sync::mpsc::channel();
    let background = m.clone();
    let worker = thread::spawn(move || tx.send(background.RequestQuota(high, 400)).unwrap());
    for _ in 0..100 {
        if larger.stopped.load(Ordering::SeqCst) != 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(larger.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(smaller.stopped.load(Ordering::SeqCst), 0);
    assert!(m.ResetRootPoolByID(1, 0, false));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        ArbitrateOk
    );
    worker.join().unwrap();
}

#[test]
fn go_merge_24_priority_request_waits_for_cancelled_pool_to_release_quota() {
    let m = Arc::new(NewMemArbitrator(1_000));
    m.SetWorkMode(ArbitratorModePriority);
    let low = m.EmplaceRootPool(1).unwrap();
    let low_helper = Arc::new(Helper::new());
    let low_ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(low_helper.clone()),
        ArbitrationPriorityLow,
        false,
        false,
    );
    assert!(m.RestartEntryByContext(low, low_ctx));
    assert_eq!(m.RequestQuota(low, 700), ArbitrateOk);
    let high = m.EmplaceRootPool(2).unwrap();
    let high_ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(Arc::new(Helper::new())),
        ArbitrationPriorityHigh,
        false,
        false,
    );
    assert!(m.RestartEntryByContext(high, high_ctx));
    let (tx, rx) = std::sync::mpsc::channel();
    let background = m.clone();
    let worker = thread::spawn(move || tx.send(background.RequestQuota(high, 400)).unwrap());
    for _ in 0..100 {
        if low_helper.stopped.load(Ordering::SeqCst) != 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(low_helper.stopped.load(Ordering::SeqCst), 1);
    assert!(rx.try_recv().is_err());
    assert!(m.ResetRootPoolByID(1, 0, false));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        ArbitrateOk
    );
    worker.join().unwrap();
}

#[test]
fn go_merge_24_cancelled_waiter_cleans_pending_accounting() {
    let m = Arc::new(NewMemArbitrator(1_000));
    m.SetWorkMode(ArbitratorModePriority);
    let low = m.EmplaceRootPool(1).unwrap();
    let low_helper = Arc::new(Helper::new());
    let low_ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(low_helper.clone()),
        ArbitrationPriorityLow,
        false,
        false,
    );
    assert!(m.RestartEntryByContext(low, low_ctx));
    assert_eq!(m.RequestQuota(low, 700), ArbitrateOk);
    let high = m.EmplaceRootPool(2).unwrap();
    let high_ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(Arc::new(Helper::new())),
        ArbitrationPriorityHigh,
        false,
        false,
    );
    assert!(m.RestartEntryByContext(high, high_ctx.clone()));
    let (tx, rx) = std::sync::mpsc::channel();
    let background = m.clone();
    let worker = thread::spawn(move || tx.send(background.RequestQuota(high, 400)).unwrap());
    for _ in 0..100 {
        if low_helper.stopped.load(Ordering::SeqCst) != 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(low_helper.stopped.load(Ordering::SeqCst), 1);
    high_ctx.stop(ArbitratorPriorityCancel);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        ArbitrateFail
    );
    worker.join().unwrap();
    assert_eq!(m.WaitingAllocSize(), 0);
    assert_eq!(m.TaskNum(), 0);
}

#[test]
fn go_merge_24_tracked_heap_caps_root_and_await_free_usage() {
    struct UsedHelper;
    impl ArbitrateHelper for UsedHelper {
        fn Stop(&self, _: ArbitratorStopReason) -> bool {
            true
        }
        fn HeapInuse(&self) -> i64 {
            400
        }
        fn MemUsage(&self) -> MemUsage {
            MemUsage {
                RootPoolUsed: 400,
                HeapInuse: 400,
            }
        }
        fn Finish(&self) {}
    }
    let m = NewMemArbitrator(1_000);
    let root = m.EmplaceRootPool(1).unwrap();
    let ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(Arc::new(UsedHelper)),
        ArbitrationPriorityLow,
        false,
        false,
    );
    assert!(m.RestartEntryByContext(root, ctx));
    assert_eq!(m.RequestQuota(root, 300), ArbitrateOk);
    assert!(m.ConsumeQuotaFromAwaitFreePool(2, 50));
    m.ReportHeapInuseToAwaitFreePool(2, 70);
    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 500,
        heap_inuse: 500,
        ..Default::default()
    });
    assert_eq!(m.OutOfControl(), 150);
}

#[test]
fn go_merge_24_auto_risk_updates_magnification_and_avoidance() {
    let m = NewMemArbitrator(1_000);
    m.SetSoftLimit(0, 0.0, SoftLimitModeAuto);
    assert!(m.allocate(200));
    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 400,
        heap_inuse: 960,
        ..Default::default()
    });
    assert_eq!(m.MemMagnifForTest(), 2_100);
    assert_eq!(m.OutOfControl(), 524);
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

#[test]
fn go_merge_24_helper_done_cancels_context_without_explicit_channel() {
    struct DoneHelper(crate::sqlkiller::SQLKiller);
    impl ArbitrateHelper for DoneHelper {
        fn Stop(&self, _: ArbitratorStopReason) -> bool {
            true
        }
        fn HeapInuse(&self) -> i64 {
            0
        }
        fn Finish(&self) {}
        fn Done(&self) -> CancelReceiver {
            CancelReceiver::from_kill_event(self.0.GetKillEventChan())
        }
    }
    let helper = Arc::new(DoneHelper(crate::sqlkiller::SQLKiller::new()));
    let ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(helper.clone()),
        ArbitrationPriorityLow,
        false,
        false,
    );
    let core = NewMemArbitrator(1_000);
    let root = core.EmplaceRootPool(82).unwrap();
    assert!(core.RestartEntryByContext(root, ctx.clone()));
    assert!(ctx.available());
    helper.0.SendKillSignal(1);
    assert!(!ctx.available());
    assert_eq!(core.RequestQuota(root, 1), ArbitrateFail);
}

#[test]
fn go_merge_24_magnification_decays_across_safe_profile_windows() {
    let core = NewMemArbitrator(10_000);
    core.SetSoftLimit(0, 0.0, SoftLimitModeAuto);
    core.SetMemMagnifForTest(2_000);
    let now = 3_000;
    assert!(!core.UpdateMemMagnificationForTest(now, now, 1_200, now, 1_000));
    assert!(!core.UpdateMemMagnificationForTest(now + 30, now + 30, 1_200, now + 30, 1_000));
    assert!(core.UpdateMemMagnificationForTest(now + 60, now + 60, 1_200, now + 60, 1_000));
    assert_eq!(core.MemMagnif(), 1_600);
}

#[test]
fn go_merge_24_avoidance_reclaims_await_free_capacity() {
    let core = NewMemArbitrator(1_000);
    let budget = core.GetAwaitFreeBudgets(17);
    assert!(core.allocate(300));
    budget.Reserve(300);
    core.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 1_000,
        heap_inuse: 100,
        ..Default::default()
    });
    assert!(budget.capacity() < 300);
    assert!(core.Allocated() < 300);
}

#[test]
fn go_merge_24_priority_reclaim_counts_inflight_cancellation() {
    let core = NewMemArbitrator(1_000);
    core.SetWorkMode(ArbitratorModePriority);
    let first = Arc::new(Helper::new());
    let second = Arc::new(Helper::new());
    for (uid, helper) in [(1, first.clone()), (2, second.clone())] {
        let root = core.EmplaceRootPool(uid).unwrap();
        let ctx = NewArbitrationContext(
            CancelReceiver::none(),
            Some(helper),
            ArbitrationPriorityLow,
            false,
            false,
        );
        assert!(core.RestartEntryByContext(root, ctx));
        assert_eq!(core.RequestQuota(root, 100), ArbitrateOk);
    }
    assert_eq!(
        core.cancel_lower_priority(ArbitrationPriorityHigh, 100),
        100
    );
    assert_eq!(
        core.cancel_lower_priority(ArbitrationPriorityHigh, 100),
        100
    );
    assert_eq!(
        first.stopped.load(Ordering::SeqCst) + second.stopped.load(Ordering::SeqCst),
        1,
    );
}
