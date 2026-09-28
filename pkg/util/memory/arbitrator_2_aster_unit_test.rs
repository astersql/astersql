// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// MemArbitrator 迁移补充单元测试。
//
// 对齐 Go：枚举名、配额分配/释放、软限制、ConcurrentBudget、
// ArbitrationContext 只停一次，以及挂起风险（memHangRisk）判定。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use super::sqlkiller::SQLKiller;
use crate::*;

#[test]
/// 优先级/模式/停止原因字符串及未知码 "UNKNOWN" 与 Go 一致。
fn enum_names_and_unknown_stop_reason_match_go() {
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
/// Limit/memRisk/oomRisk、allocate/release 与 SetLimit 行为对齐 Go。
fn limits_allocation_and_risk_thresholds_follow_go() {
    let mut m = NewMemArbitrator(100);
    assert_eq!(m.Limit(), 100);
    assert_eq!(m.memRisk(), 90);
    assert_eq!(m.oomRisk(), 95);
    assert!(m.allocate(60));
    assert!(!m.allocate(50));
    assert_eq!(m.Allocated(), 60);
    m.release(20);
    assert_eq!(m.Allocated(), 40);
    assert!(!m.SetLimit(100));
    assert!(m.SetLimit(200));
    assert_eq!(m.Limit(), 200);
}

#[test]
/// SoftLimitModeSpecified 支持绝对字节与比例；Disable 回落到 oomRisk。
fn specified_soft_limit_supports_size_and_ratio() {
    let mut m = NewMemArbitrator(1_000);
    m.SetSoftLimit(600, 0.0, SoftLimitModeSpecified);
    assert_eq!(m.SoftLimit(), 600);
    m.SetSoftLimit(0, 0.75, SoftLimitModeSpecified);
    assert_eq!(m.SoftLimit(), 750);
    m.SetSoftLimit(0, 0.0, SoftLimitModeDisable);
    assert_eq!(m.SoftLimit(), 950);
}

#[test]
/// ConcurrentBudget 记录最近使用时间，超额返回 BudgetExhausted。
fn concurrent_budget_tracks_time_usage_and_overflow() {
    let budget = ConcurrentBudget::new(10);
    assert!(budget.ConsumeQuota(7, 7).is_ok());
    assert_eq!(budget.used(), 7);
    assert_eq!(budget.last_used_time_sec(), 7);
    assert!(budget.ConsumeQuota(8, 4).is_err());
    assert_eq!(budget.used(), 11);
    assert!(budget.ConsumeQuota(8, -5).is_ok());
    assert_eq!(budget.used(), 6);
}

/// 测试用 ArbitrateHelper：Stop 调用计数。
struct Helper(AtomicUsize);

impl ArbitrateHelper for Helper {
    fn Stop(&self, _reason: ArbitratorStopReason) -> bool {
        self.0.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn HeapInuse(&self) -> i64 {
        0
    }
    fn Finish(&self) {}
}

#[test]
/// ArbitrationContext.stop 只对 helper 生效一次。
fn arbitration_context_stops_helper_only_once() {
    let helper = Arc::new(Helper(AtomicUsize::new(0)));
    let ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(helper.clone()),
        ArbitrationPriorityHigh,
        true,
        false,
    );
    assert!(ctx.available());
    ctx.stop(ArbitratorOOMRiskKill);
    ctx.stop(ArbitratorOOMRiskKill);
    assert!(!ctx.available());
    assert_eq!(helper.0.load(Ordering::SeqCst), 1);
}

#[test]
fn arbitration_context_observes_session_cancel_channel() {
    let killer = SQLKiller::new();
    let ctx = NewArbitrationContext(
        CancelReceiver::from_kill_event(killer.GetKillEventChan()),
        Some(Arc::new(Helper(AtomicUsize::new(0)))),
        ArbitrationPriorityMedium,
        false,
        false,
    );
    assert!(ctx.available());
    killer.SendKillSignal(1);
    assert!(!ctx.available());
}

#[test]
fn root_pool_digest_await_free_and_runtime_paths_match_go() {
    let m = NewMemArbitrator(10_000);
    m.SetWorkMode(ArbitratorModePriority);
    let root = m.EmplaceRootPool(42).unwrap();
    assert_eq!(m.RootPoolNum(), 1);

    let ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(Arc::new(Helper(AtomicUsize::new(0)))),
        ArbitrationPriorityHigh,
        false,
        true,
    );
    assert!(m.RestartEntryByContext(root, ctx));
    assert_eq!(m.RequestQuota(root, 2_000), ArbitrateOk);
    assert_eq!(m.Allocated(), 2_000);

    m.UpdateDigestProfileCache(7, 1_024, 10);
    m.UpdateDigestProfileCache(7, 2_048, 11);
    assert_eq!(m.GetDigestProfileCache(7, 12), Some(2_048));

    assert!(m.ConsumeQuotaFromAwaitFreePool(42, 128));
    m.ReportHeapInuseToAwaitFreePool(42, 64);
    let await_free = m.GetAwaitFreeBudgets(42);
    assert_eq!(await_free.used(), 128);
    assert_eq!(await_free.heap_inuse(), 64);

    m.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 9_100,
        heap_inuse: 10_001,
        ..ArbitratorRuntimeStats::default()
    });
    assert!(m.AtMemRisk());
    assert!(m.AtOOMRisk());

    assert!(m.ResetRootPoolByID(42, 2_000, true));
    assert!(m.ConsumeQuotaFromAwaitFreePool(42, -128));
    assert_eq!(m.ShrinkAwaitFreePool(0), 128);
    assert_eq!(m.Allocated(), 0);
    assert!(m.RemoveRootPoolByID(42));
    assert_eq!(m.RootPoolNum(), 0);
}

#[test]
/// 工作模式、任务计数、堆风险阈值与缓存 Unix 时间联动正确。
fn mode_tasks_risk_and_cached_time_change_consistently() {
    let mut m = NewMemArbitrator(100);
    assert_eq!(m.WorkMode(), ArbitratorModeDisable);
    assert!(m.SetWorkMode(ArbitratorModePriority));
    assert!(!m.SetWorkMode(ArbitratorModePriority));
    m.record_task(ArbitrationPriorityLow, false);
    m.record_task(ArbitrationPriorityHigh, true);
    assert_eq!(m.TaskNumByPattern(), [1, 0, 1, 1]);

    m.set_runtime_heap(89);
    assert!(!m.AtMemRisk());
    m.set_runtime_heap(90);
    assert!(m.AtMemRisk());
    assert!(!m.AtOOMRisk());
    m.set_runtime_heap(95);
    assert!(m.AtOOMRisk());

    m.setUnixTimeSec(1_725_000_123);
    assert_eq!(m.approxUnixTimeSec(), 1_725_000_123);
}

#[test]
/// 释放速度过慢或超过 5s 回收检查窗口即判定挂起风险。
fn memory_hang_risk_uses_speed_or_strict_five_second_timeout() {
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

#[test]
fn execution_metrics_track_success_priority_and_cancel_reasons() {
    let standard = NewMemArbitrator(100);
    standard.SetWorkMode(ArbitratorModeStandard);

    let first = standard.EmplaceRootPool(1).unwrap();
    assert!(standard.RestartEntryByContext(
        first,
        NewArbitrationContext(
            CancelReceiver::none(),
            Some(Arc::new(Helper(AtomicUsize::new(0)))),
            ArbitrationPriorityLow,
            false,
            false,
        ),
    ));
    assert_eq!(standard.RequestQuota(first, 60), ArbitrateOk);

    let second = standard.EmplaceRootPool(2).unwrap();
    assert!(standard.RestartEntryByContext(
        second,
        NewArbitrationContext(
            CancelReceiver::none(),
            Some(Arc::new(Helper(AtomicUsize::new(0)))),
            ArbitrationPriorityHigh,
            false,
            false,
        ),
    ));
    assert_eq!(standard.RequestQuota(second, 50), ArbitrateFail);

    let third = standard.EmplaceRootPool(3).unwrap();
    assert!(standard.RestartEntryByContext(
        third,
        NewArbitrationContext(
            CancelReceiver::none(),
            Some(Arc::new(Helper(AtomicUsize::new(0)))),
            ArbitrationPriorityMedium,
            true,
            false,
        ),
    ));
    assert_eq!(standard.RequestQuota(third, 50), ArbitrateFail);

    let metrics = standard.ExecMetrics();
    assert_eq!(metrics.Task.Succ, 1);
    assert_eq!(metrics.Task.Fail, 2);
    assert_eq!(metrics.Task.SuccByPriority, [0, 0, 0]);
    assert_eq!(metrics.Cancel.StandardMode, 1);
    assert_eq!(metrics.Cancel.WaitAverse, 1);

    let priority = NewMemArbitrator(100);
    priority.SetWorkMode(ArbitratorModePriority);
    let root = priority.EmplaceRootPool(4).unwrap();
    assert!(priority.RestartEntryByContext(
        root,
        NewArbitrationContext(
            CancelReceiver::none(),
            Some(Arc::new(Helper(AtomicUsize::new(0)))),
            ArbitrationPriorityHigh,
            false,
            false,
        ),
    ));
    assert_eq!(priority.RequestQuota(root, 10), ArbitrateOk);
    assert_eq!(priority.ExecMetrics().Task.SuccByPriority, [0, 0, 1]);
}
