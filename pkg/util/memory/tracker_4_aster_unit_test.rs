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

// Tracker 核心语义的 Aster 单元测试。
//
// 覆盖父子消费传播与 Detach、全局 Tracker 缓冲记账、软/硬限动作阈值、
// 并发 Consume、ReplaceChild 与族谱统计，以及 FormatBytes。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::tracker::{
    ActionOnExceed, FormatBytes, NewGlobalTracker, NewTracker, TrackMemWhenExceeds, Tracker,
};
#[cfg(feature = "mem-arbitrator")]
use super::{ArbitrationPriorityMedium, ArbitratorModeStandard, NewMemArbitrator};

/// 计数型超限动作：每次 Action 原子 +1，可挂 fallback。
struct CountingAction {
    calls: Arc<AtomicUsize>,
    fallback: Option<Box<dyn ActionOnExceed>>,
    finished: bool,
}

impl CountingAction {
    fn new(calls: Arc<AtomicUsize>) -> Self {
        Self {
            calls,
            fallback: None,
            finished: false,
        }
    }
}

impl ActionOnExceed for CountingAction {
    fn Action(&mut self, _tracker: &mut Tracker) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
    fn SetFallback(&mut self, action: Option<Box<dyn ActionOnExceed>>) {
        self.fallback = action;
    }
    fn GetFallback(&mut self) -> Option<Box<dyn ActionOnExceed>> {
        self.fallback.take()
    }
    fn GetPriority(&self) -> i64 {
        0
    }
    fn SetFinished(&mut self) {
        self.finished = true;
    }
    fn IsFinished(&self) -> bool {
        self.finished
    }
}

#[test]
/// Attach 后子消费上卷到父；Detach 后父清零、子保留本地消费。
fn tracker_tree_propagates_consumption_and_detach_matches_go() {
    let mut parent = NewTracker(1, -1);
    let mut child = NewTracker(2, -1);

    child.Consume(100);
    child.AttachTo(&mut *parent);
    assert_eq!(child.BytesConsumed(), 100);
    assert_eq!(parent.BytesConsumed(), 100);
    assert_eq!(parent.GetChildrenForTest().len(), 1);

    child.Consume(25);
    assert_eq!(parent.BytesConsumed(), 125);
    assert_eq!(parent.MaxConsumed(), 125);

    child.Detach();
    assert_eq!(child.BytesConsumed(), 125);
    assert_eq!(parent.BytesConsumed(), 0);
    assert!(parent.GetChildrenForTest().is_empty());
}

#[test]
/// 全局 Tracker + BufferedConsume：累计达阈值才真正记账。
fn global_tracker_and_buffered_accounting_match_go() {
    let mut global = NewGlobalTracker(10, -1);
    let mut child = NewTracker(11, -1);
    child.AttachToGlobalTracker(&mut *global);

    let mut buffered = 0;
    child.BufferedConsume(&mut buffered, TrackMemWhenExceeds / 2);
    assert_eq!(child.BytesConsumed(), 0);
    child.BufferedConsume(&mut buffered, TrackMemWhenExceeds / 2);
    assert_eq!(child.BytesConsumed(), TrackMemWhenExceeds);
    assert_eq!(global.BytesConsumed(), TrackMemWhenExceeds);

    child.DetachFromGlobalTracker();
    assert_eq!(global.BytesConsumed(), 0);
}

#[test]
/// 硬限检查、峰值 MaxConsumed/Reset 与 FormatBytes 单位。
fn limits_peak_reset_and_formatting_match_go() {
    let tracker = NewTracker(1, 100);
    assert_eq!(tracker.GetBytesLimit(), 100);
    assert!(!tracker.CheckExceed());
    tracker.Consume(100);
    assert!(tracker.CheckExceed());
    assert_eq!(tracker.MaxConsumed(), 100);
    tracker.Consume(-40);
    assert_eq!(tracker.MaxConsumed(), 100);
    tracker.ResetMaxConsumed();
    assert_eq!(tracker.MaxConsumed(), 60);

    assert_eq!(FormatBytes(100), "100 Bytes");
    assert_eq!(FormatBytes(2 * 1024), "2 KB");
    assert_eq!(FormatBytes(3 * 1024 * 1024), "3 MB");
    assert_eq!(FormatBytes(4_i64 * 1024 * 1024 * 1024), "4 GB");
}

#[test]
/// 多线程正负 Consume 后净额与峰值区间符合原子记账。
fn concurrent_consume_matches_go_atomic_accounting() {
    let tracker: Arc<Tracker> = Arc::from(NewTracker(1, -1));
    tracker.Consume(100);
    let mut workers = Vec::new();
    for delta in [10_i64, -10].into_iter().cycle().take(20) {
        let tracker = Arc::clone(&tracker);
        workers.push(std::thread::spawn(move || tracker.Consume(delta)));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(tracker.BytesConsumed(), 100);
    assert!((100..=200).contains(&tracker.MaxConsumed()));
}

#[test]
/// ReplaceChild、按 label 搜索与族谱内存统计。
fn replace_search_and_family_tree_match_go() {
    let mut parent = NewTracker(1, -1);
    let mut old_child = NewTracker(2, -1);
    let mut new_child = NewTracker(2, -1);
    old_child.Consume(100);
    new_child.Consume(500);
    old_child.AttachTo(&mut *parent);

    parent.ReplaceChild(&mut *old_child, &mut *new_child);
    assert_eq!(parent.BytesConsumed(), 500);
    assert_eq!(
        parent.SearchTrackerWithoutLock(2),
        &mut *new_child as *mut Tracker
    );
    assert_eq!(parent.SearchTrackerConsumedMoreThanNBytes(499).len(), 1);
    assert_eq!(
        parent.CountAllChildrenMemUse().get("[1] <- [2]"),
        Some(&500)
    );
}

#[test]
/// 软限约 0.8*硬限先触发，再触硬限动作。
fn hard_and_soft_limit_actions_follow_go_thresholds() {
    let tracker = NewTracker(1, 100);
    let hard_calls = Arc::new(AtomicUsize::new(0));
    let soft_calls = Arc::new(AtomicUsize::new(0));
    tracker.SetActionOnExceed(Some(Box::new(CountingAction::new(Arc::clone(&hard_calls)))));
    tracker.FallbackOldAndSetNewActionForSoftLimit(Some(Box::new(CountingAction::new(
        Arc::clone(&soft_calls),
    ))));

    tracker.Consume(80);
    assert_eq!(soft_calls.load(Ordering::SeqCst), 1);
    assert_eq!(hard_calls.load(Ordering::SeqCst), 0);
    tracker.Consume(20);
    assert_eq!(soft_calls.load(Ordering::SeqCst), 2);
    assert_eq!(hard_calls.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "mem-arbitrator")]
#[test]
fn tracker_promotes_small_budget_to_shared_root_pool_and_cleans_up() {
    let core = Arc::new(NewMemArbitrator(1_000));
    core.SetWorkMode(ArbitratorModeStandard);
    let mut tracker = NewTracker(1, -1);
    tracker.IsRootTrackerOfSess = true;
    tracker.SessionID.Store(88);
    assert!(tracker.InitMemArbitrator(
        Some(core.clone()),
        None,
        "select-1",
        ArbitrationPriorityMedium,
        false,
        0,
        false,
    ));

    tracker.Consume(10);
    tracker.Consume(5);
    assert_eq!(tracker.BytesConsumed(), 15);
    assert_eq!(core.RootPoolNum(), 1);
    assert!(core.Allocated() >= 15);

    assert!(tracker.DetachMemArbitrator(false));
    core.ShrinkAwaitFreePool(0);
    assert_eq!(core.Allocated(), 0);
    assert_eq!(
        core.GetDigestProfileCache(super::HashStr("select-1"), 1),
        Some(15)
    );
}
