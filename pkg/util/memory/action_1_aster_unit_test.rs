// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 内存超限动作迁移补充单元测试。
//
// 覆盖优先级包装、fallback 链跳过 finished 节点、LogOnExceed 并发只触发一次，
// 以及 PanicOnExceed 杀查询信号与日志钩子行为。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::action::{
    ActionHandle, ActionOnExceed, BaseOOMAction, DefCursorFetchSpillPriority, DefLogPriority,
    DefPanicPriority, DefRateLimitPriority, DefSpillPriority, LogOnExceed, NewActionWithPriority,
    PanicOnExceed,
};
use crate::sqlkiller::{QueryMemoryExceeded, SQLKiller};
use crate::tracker::Tracker;

/// 测试用动作：统计调用次数，第二次起触发 fallback。
struct CountingAction {
    base: BaseOOMAction,
    calls: Arc<AtomicUsize>,
    priority: i64,
}

impl CountingAction {
    /// 按指定优先级构造计数动作。
    fn new(priority: i64) -> Self {
        Self {
            base: BaseOOMAction::default(),
            calls: Arc::new(AtomicUsize::new(0)),
            priority,
        }
    }
}

impl ActionOnExceed for CountingAction {
    fn Action(&self, tracker: &Tracker) {
        if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
            self.base.TriggerFallBackAction(tracker);
        }
    }

    fn SetFallback(&self, action: Option<ActionHandle>) {
        self.base.SetFallback(action);
    }

    fn GetFallback(&self) -> Option<ActionHandle> {
        self.base.GetFallback()
    }

    fn GetPriority(&self) -> i64 {
        self.priority
    }

    fn SetFinished(&self) {
        self.base.SetFinished();
    }

    fn IsFinished(&self) -> bool {
        self.base.IsFinished()
    }
}

/// 将 CountingAction 包装为可共享的 ActionHandle。
fn handle(action: CountingAction) -> ActionHandle {
    Arc::new(Mutex::new(action))
}

#[test]
/// 校验默认优先级常量与 NewActionWithPriority 转发/覆盖行为。
fn priority_wrapper_forwards_behavior_and_overrides_priority() {
    assert_eq!(
        [
            DefPanicPriority,
            DefLogPriority,
            DefSpillPriority,
            DefCursorFetchSpillPriority,
            DefRateLimitPriority,
        ],
        [0, 1, 2, 3, 4]
    );

    let inner_action = CountingAction::new(91);
    let inner_calls = inner_action.calls.clone();
    let inner = handle(inner_action);
    let wrapped = NewActionWithPriority(inner.clone(), 7);
    wrapped.Action(&Tracker::new(1, 100));
    assert_eq!(wrapped.GetPriority(), 7);
    assert_eq!(inner_calls.load(Ordering::SeqCst), 1);
    wrapped.SetFinished();
    assert!(inner.lock().unwrap().IsFinished());
}

#[test]
/// GetFallback 应跳过已 finished 节点并触发首个仍存活的动作。
fn fallback_chain_skips_finished_actions_and_triggers_first_live_action() {
    let first = handle(CountingAction::new(1));
    let second = handle(CountingAction::new(2));
    let third_action = CountingAction::new(3);
    let third_calls = third_action.calls.clone();
    let third = handle(third_action);
    first.lock().unwrap().SetFallback(Some(second.clone()));
    second.lock().unwrap().SetFallback(Some(third.clone()));
    first.lock().unwrap().SetFinished();
    second.lock().unwrap().SetFinished();

    let base = BaseOOMAction::default();
    base.SetFallback(Some(first));
    let live = base.GetFallback().expect("third action remains live");
    assert!(Arc::ptr_eq(&live, &third));
    base.TriggerFallBackAction(&Tracker::new(1, 100));
    assert_eq!(third_calls.load(Ordering::SeqCst), 1);
}

#[test]
/// 并发多次 Action 时 LogOnExceed 钩子只应调用一次。
fn log_action_calls_hook_only_once_under_concurrency() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen_conn = Arc::new(AtomicUsize::new(0));
    let action = Arc::new(LogOnExceed::new(42));
    action.SetLogHook({
        let calls = calls.clone();
        let seen_conn = seen_conn.clone();
        Box::new(move |conn_id| {
            calls.fetch_add(1, Ordering::SeqCst);
            seen_conn.store(conn_id as usize, Ordering::SeqCst);
        })
    });

    let mut threads = Vec::new();
    for _ in 0..12 {
        let action = action.clone();
        threads.push(thread::spawn(move || action.Action(&Tracker::new(9, 10))));
    }
    for thread in threads {
        thread.join().unwrap();
    }

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(seen_conn.load(Ordering::SeqCst), 42);
    assert_eq!(action.GetPriority(), DefLogPriority);
}

#[test]
/// PanicOnExceed：钩子一次 + 发送 QueryMemoryExceeded，HandleSignal 以 panic 抛出。
fn panic_action_logs_once_and_sends_memory_kill_signal() {
    let killer = Arc::new(SQLKiller::new());
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let action = PanicOnExceed::new(killer.clone(), 88);
    action.SetLogHook({
        let hook_calls = hook_calls.clone();
        Box::new(move |conn_id| {
            assert_eq!(conn_id, 88);
            hook_calls.fetch_add(1, Ordering::SeqCst);
        })
    });
    let tracker = Tracker::new(5, 10);

    for _ in 0..2 {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            action.Action(&tracker);
        }));
        assert!(
            result.is_err(),
            "HandleSignal error must be raised as a panic"
        );
    }

    assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
    assert_eq!(killer.GetKillSignal(), QueryMemoryExceeded);
    assert_eq!(action.GetPriority(), DefPanicPriority);
}
