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

// Tracker 与全局内存仲裁器的集成风格单元测试。
//
// 对应 Go `tracker_test.go`：标签、Consume/Release、缓冲记账、OOM 动作链、
// Attach/Detach/Replace、ToString、全局 Tracker、FormatBytes、动作优先级，
// 以及 GlobalMemArbitrator 工作模式与软限制。

use super::arbitrator::DefMaxLimit;
#[cfg(feature = "mem-arbitrator")]
use super::arbitrator::{ArbitrationPriorityMedium, NewMemArbitrator};
use super::global_arbitrator::{
    CleanupGlobalMemArbitratorForTest, GlobalMemArbitrator, RemovePoolFromGlobalMemArbitrator,
    RuntimeMemStateRecorder, SetGlobalMemArbitratorLimit, SetGlobalMemArbitratorSoftLimit,
    SetGlobalMemArbitratorWorkMode, SetupGlobalMemArbitratorForTest, SoftLimitMode, WorkMode,
    parse_soft_limit,
};
use super::tracker::{
    ActionOnExceed, EnableGCAwareMemoryTrack, FormatBytes, NewGlobalTracker, NewTracker,
    TrackMemWhenExceeds, Tracker,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// 取得 Box<Tracker> 内对象的裸指针，便于树 API 指针比较。
fn ptr(tracker: &mut Box<Tracker>) -> *mut Tracker {
    &mut **tracker
}

#[test]
#[cfg(feature = "mem-arbitrator")]
fn go_merge_24_tracker_helper_reports_root_pool_and_heap_usage() {
    let core = Arc::new(NewMemArbitrator(10_000));
    let mut tracker = NewTracker(1, -1);
    assert!(tracker.InitMemArbitrator(
        Some(core),
        None,
        0,
        ArbitrationPriorityMedium,
        false,
        1_000,
        false,
    ));
    tracker.Consume(100);
    let usage = tracker.MemArbitrator.as_ref().unwrap().MemUsage();
    assert_eq!(usage.RootPoolUsed, 100);
    assert_eq!(usage.HeapInuse, 100);
}

#[derive(Default)]
/// MockAction 共享调用计数。
struct ActionState {
    calls: AtomicUsize,
}

/// 可配置优先级与 fallback 的测试用超限动作。
struct MockAction {
    state: Arc<ActionState>,
    fallback: Option<Box<dyn ActionOnExceed>>,
    priority: i64,
    finished: AtomicBool,
}

impl MockAction {
    fn new(priority: i64) -> (Box<Self>, Arc<ActionState>) {
        let state = Arc::new(ActionState::default());
        (
            Box::new(Self {
                state: state.clone(),
                fallback: None,
                priority,
                finished: AtomicBool::new(false),
            }),
            state,
        )
    }
}

impl ActionOnExceed for MockAction {
    fn Action(&mut self, tracker: &mut Tracker) {
        if self.state.calls.fetch_add(1, Ordering::SeqCst) > 0 {
            if let Some(fallback) = self.fallback.as_mut() {
                fallback.Action(tracker);
            }
        }
    }

    fn SetFallback(&mut self, action: Option<Box<dyn ActionOnExceed>>) {
        self.fallback = action;
    }

    fn GetFallback(&mut self) -> Option<Box<dyn ActionOnExceed>> {
        self.fallback.take()
    }

    fn GetPriority(&self) -> i64 {
        self.priority
    }

    fn SetFinished(&mut self) {
        self.finished.store(true, Ordering::SeqCst);
    }

    fn IsFinished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }
}

#[test]
/// SetLabel 不改变消费与子女列表。
fn TestSetLabel() {
    let mut tracker = NewTracker(1, -1);
    assert_eq!(tracker.Label(), 1);
    assert_eq!(tracker.BytesConsumed(), 0);
    assert_eq!(tracker.GetBytesLimit(), -1);
    assert!(tracker.GetChildrenForTest().is_empty());
    tracker.SetLabel(2);
    assert_eq!(tracker.Label(), 2);
    assert_eq!(tracker.BytesConsumed(), 0);
    assert_eq!(tracker.GetBytesLimit(), -1);
    assert!(tracker.GetChildrenForTest().is_empty());
}

#[test]
/// 已 Attach 子节点改 label 后父侧索引更新。
fn TestSetLabel2() {
    let mut tracker = NewTracker(1, -1);
    let mut tracker2 = NewTracker(2, -1);
    tracker2.AttachTo(ptr(&mut tracker));
    tracker2.Consume(10);
    assert_eq!(tracker.BytesConsumed(), 10);
    tracker2.SetLabel(10);
    assert_eq!(tracker.BytesConsumed(), 10);
    tracker2.Detach();
    assert_eq!(tracker.BytesConsumed(), 0);
}

#[test]
/// 父子 Consume 累加与负值释放。
fn TestConsume() {
    let tracker = Arc::new(NewTracker(1, -1));
    tracker.Consume(100);
    let mut threads = Vec::new();
    for _ in 0..10 {
        let tracker = tracker.clone();
        threads.push(std::thread::spawn(move || tracker.Consume(10)));
    }
    for _ in 0..10 {
        let tracker = tracker.clone();
        threads.push(std::thread::spawn(move || tracker.Consume(-10)));
    }
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(tracker.BytesConsumed(), 100);
}

#[test]
/// Release / GCAware 释放路径。
fn TestRelease() {
    let mut parent = NewGlobalTracker(super::tracker::LabelForGlobalAnalyzeMemory, -1);
    let mut tracker = NewTracker(1, -1);
    tracker.AttachToGlobalTracker(ptr(&mut parent));
    EnableGCAwareMemoryTrack.Store(false);
    tracker.Consume(100);
    tracker.Release(100);
    assert_eq!((tracker.BytesConsumed(), parent.BytesConsumed()), (0, 0));
    assert_eq!((tracker.BytesReleased(), parent.BytesReleased()), (0, 0));

    EnableGCAwareMemoryTrack.Store(true);
    tracker.Consume(100);
    tracker.Release(100);
    // Rust has no Go finalizer; accounting completes synchronously at Release.
    assert_eq!((tracker.BytesConsumed(), parent.BytesConsumed()), (0, 0));
    assert_eq!((tracker.BytesReleased(), parent.BytesReleased()), (0, 0));
    EnableGCAwareMemoryTrack.Store(false);
}

#[test]
/// 缓冲记账达阈值才真正 Consume/Release。
fn TestBufferedConsumeAndRelease() {
    let mut parent = NewGlobalTracker(super::tracker::LabelForGlobalAnalyzeMemory, -1);
    let mut tracker = NewTracker(1, -1);
    tracker.AttachToGlobalTracker(ptr(&mut parent));
    EnableGCAwareMemoryTrack.Store(true);
    let mut buffered = 0;
    tracker.BufferedConsume(&mut buffered, TrackMemWhenExceeds / 2);
    assert_eq!(tracker.BytesConsumed(), 0);
    tracker.BufferedConsume(&mut buffered, TrackMemWhenExceeds / 2);
    assert_eq!(tracker.BytesConsumed(), TrackMemWhenExceeds);
    let mut released = 0;
    tracker.BufferedRelease(&mut released, TrackMemWhenExceeds / 2);
    assert_eq!(parent.BytesConsumed(), TrackMemWhenExceeds);
    tracker.BufferedRelease(&mut released, TrackMemWhenExceeds / 2);
    assert_eq!((parent.BytesConsumed(), parent.BytesReleased()), (0, 0));
    EnableGCAwareMemoryTrack.Store(false);
}

#[test]
/// 硬限触发 OOM 动作及 fallback 链。
fn TestOOMAction() {
    let tracker = NewTracker(1, 100);
    tracker.Consume(10_000);

    let tracker = NewTracker(1, 100);
    let (action, state) = MockAction::new(0);
    tracker.SetActionOnExceed(Some(action));
    tracker.Consume(10_000);
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);

    let (action1, state1) = MockAction::new(0);
    let (action2, state2) = MockAction::new(0);
    tracker.SetActionOnExceed(Some(action1));
    tracker.FallbackOldAndSetNewAction(Some(action2));
    tracker.Consume(10_000);
    assert_eq!(
        (
            state2.calls.load(Ordering::SeqCst),
            state1.calls.load(Ordering::SeqCst)
        ),
        (1, 0)
    );
    tracker.Consume(10_000);
    assert_eq!(
        (
            state2.calls.load(Ordering::SeqCst),
            state1.calls.load(Ordering::SeqCst)
        ),
        (2, 1)
    );

    let tracker = NewTracker(1, 100);
    let (hard, hard_state) = MockAction::new(0);
    let (soft1, soft1_state) = MockAction::new(0);
    let (soft2, soft2_state) = MockAction::new(0);
    tracker.SetActionOnExceed(Some(hard));
    tracker.FallbackOldAndSetNewActionForSoftLimit(Some(soft1));
    tracker.FallbackOldAndSetNewActionForSoftLimit(Some(soft2));
    tracker.Consume(80);
    assert_eq!(
        (
            soft2_state.calls.load(Ordering::SeqCst),
            soft1_state.calls.load(Ordering::SeqCst),
            hard_state.calls.load(Ordering::SeqCst)
        ),
        (1, 0, 0)
    );
    tracker.Consume(20);
    assert_eq!(
        (
            soft2_state.calls.load(Ordering::SeqCst),
            soft1_state.calls.load(Ordering::SeqCst),
            hard_state.calls.load(Ordering::SeqCst)
        ),
        (2, 1, 1)
    );
}

#[test]
/// AttachTo 建立父子并上卷已有消费。
fn TestAttachTo() {
    let mut old_parent = NewTracker(1, -1);
    let mut new_parent = NewTracker(2, -1);
    let mut child = NewTracker(3, -1);
    child.Consume(100);
    child.AttachTo(ptr(&mut old_parent));
    assert_eq!(old_parent.BytesConsumed(), 100);
    assert_eq!(old_parent.GetChildrenForTest(), vec![ptr(&mut child)]);
    child.AttachTo(ptr(&mut new_parent));
    assert_eq!(
        (old_parent.BytesConsumed(), new_parent.BytesConsumed()),
        (0, 100)
    );
    assert!(old_parent.GetChildrenForTest().is_empty());
    assert_eq!(new_parent.GetChildrenForTest(), vec![ptr(&mut child)]);
}

#[test]
/// Detach 解除父子并回滚父侧消费。
fn TestDetach() {
    let mut parent = NewTracker(1, -1);
    let mut child = NewTracker(2, -1);
    child.Consume(100);
    child.AttachTo(ptr(&mut parent));
    child.Detach();
    assert_eq!(child.BytesConsumed(), 100);
    assert_eq!(parent.BytesConsumed(), 0);
    assert!(parent.GetChildrenForTest().is_empty());
}

#[test]
/// ReplaceChild 替换子节点并调整父消费。
fn TestReplaceChild() {
    let mut old_child = NewTracker(1, -1);
    let mut new_child = NewTracker(2, -1);
    let mut parent = NewTracker(3, -1);
    old_child.Consume(100);
    new_child.Consume(500);
    old_child.AttachTo(ptr(&mut parent));
    parent.ReplaceChild(ptr(&mut old_child), ptr(&mut new_child));
    assert_eq!(parent.BytesConsumed(), 500);
    assert_eq!(parent.GetChildrenForTest(), vec![ptr(&mut new_child)]);
    parent.ReplaceChild(ptr(&mut old_child), std::ptr::null_mut());
    assert_eq!(parent.BytesConsumed(), 500);
    parent.ReplaceChild(ptr(&mut new_child), std::ptr::null_mut());
    assert_eq!(parent.BytesConsumed(), 0);

    let mut node1 = NewTracker(1, -1);
    let mut node2 = NewTracker(2, -1);
    let mut node3 = NewTracker(3, -1);
    node2.AttachTo(ptr(&mut node1));
    node3.AttachTo(ptr(&mut node2));
    node3.Consume(100);
    node2.ReplaceChild(ptr(&mut node3), std::ptr::null_mut());
    assert_eq!((node2.BytesConsumed(), node1.BytesConsumed()), (0, 0));
}

#[test]
/// String/ToString 展示树结构。
fn TestToString() {
    let mut parent = NewTracker(1, -1);
    let mut children = [
        NewTracker(2, 1000),
        NewTracker(3, -1),
        NewTracker(4, -1),
        NewTracker(5, -1),
    ];
    for child in &mut children {
        child.AttachTo(ptr(&mut parent));
    }
    children[0].Consume(100);
    children[1].Consume(2 * 1024);
    children[2].Consume(3 * 1024 * 1024);
    children[3].Consume(4_i64 * 1024 * 1024 * 1024);
    assert_eq!(
        parent.String(),
        "\n\"1\"{\n  \"consumed\": 4.00 GB\n  \"2\"{\n    \"quota\": 1000 Bytes\n    \"consumed\": 100 Bytes\n  }\n  \"3\"{\n    \"consumed\": 2 KB\n  }\n  \"4\"{\n    \"consumed\": 3 MB\n  }\n  \"5\"{\n    \"consumed\": 4 GB\n  }\n}\n"
    );
}

#[test]
/// MaxConsumed 记录峰值。
fn TestMaxConsumed() {
    let mut root = NewTracker(1, -1);
    let mut child = NewTracker(2, -1);
    child.AttachTo(ptr(&mut root));
    for delta in [100, 200, -50, 500, -750, 25, -10, 35, -50, 10] {
        child.Consume(delta);
    }
    assert_eq!(root.BytesConsumed(), 10);
    assert_eq!(root.MaxConsumed(), 750);
    root.ResetMaxConsumed();
    assert_eq!(root.MaxConsumed(), 10);
}

#[test]
/// 全局 Tracker Attach/Detach 语义。
fn TestGlobalTracker() {
    let mut root = NewGlobalTracker(1, -1);
    let mut c1 = NewTracker(2, -1);
    let mut c2 = NewTracker(3, -1);
    c1.Consume(100);
    c2.Consume(200);
    c1.AttachToGlobalTracker(ptr(&mut root));
    c2.AttachToGlobalTracker(ptr(&mut root));
    assert_eq!(root.BytesConsumed(), 300);
    assert!(root.GetChildrenForTest().is_empty());
    c1.DetachFromGlobalTracker();
    c2.DetachFromGlobalTracker();
    assert_eq!(root.BytesConsumed(), 0);
    let mut common = NewTracker(4, -1);
    let result = catch_unwind(AssertUnwindSafe(|| {
        c1.AttachToGlobalTracker(ptr(&mut common))
    }));
    assert!(result.is_err());
}

/// 解析带单位的字节字符串，供 FormatBytes 用例。
fn parse_byte(value: &str) -> Result<i64, String> {
    let split = value
        .find(|c: char| c.is_ascii_alphabetic())
        .ok_or_else(|| "missing unit".to_owned())?;
    let number: f64 = value[..split]
        .trim()
        .parse::<f64>()
        .map_err(|error| error.to_string())?;
    let unit = match value[split..].trim() {
        "GB" => 1024_i64.pow(3),
        "MB" => 1024_i64.pow(2),
        "KB" => 1024,
        "Bytes" => 1,
        other => return Err(format!("invalid byte unit: {other}")),
    };
    Ok((number * unit as f64) as i64)
}

#[test]
/// FormatBytes 修剪与单位选择。
fn TestFormatBytesWithPrune() {
    let cases = [
        ("0 Bytes", "0 Bytes"),
        ("1 Bytes", "1 Bytes"),
        ("9 Bytes", "9 Bytes"),
        ("10 Bytes", "10 Bytes"),
        ("999 Bytes", "999 Bytes"),
        ("1 KB", "1024 Bytes"),
        ("1.123 KB", "1.12 KB"),
        ("1.023 KB", "1.02 KB"),
        ("1.003 KB", "1.00 KB"),
        ("10.456 KB", "10.5 KB"),
        ("10.956 KB", "11.0 KB"),
        ("999.056 KB", "999.1 KB"),
        ("999.988 KB", "1000.0 KB"),
        ("1.123 MB", "1.12 MB"),
        ("1.023 MB", "1.02 MB"),
        ("1.003 MB", "1.00 MB"),
        ("10.456 MB", "10.5 MB"),
        ("10.956 MB", "11.0 MB"),
        ("999.056 MB", "999.1 MB"),
        ("999.988 MB", "1000.0 MB"),
        ("1.123 GB", "1.12 GB"),
        ("1.023 GB", "1.02 GB"),
        ("1.003 GB", "1.00 GB"),
        ("10.456 GB", "10.5 GB"),
        ("10.956 GB", "11.0 GB"),
        ("9.412345 MB", "9.41 MB"),
        ("10.412345 MB", "10.4 MB"),
        ("5.999 GB", "6.00 GB"),
        ("100.46 KB", "100.5 KB"),
        ("18.399999618530273 MB", "18.4 MB"),
        ("9.15999984741211 MB", "9.16 MB"),
    ];
    for (input, expected) in cases {
        assert_eq!(
            FormatBytes(parse_byte(input).unwrap()),
            expected,
            "input: {input}"
        );
    }
    assert!(parse_byte("1 XB").is_err());
}

#[test]
/// 错误码相关占位/断言。
fn TestErrorCode() {
    assert_eq!(super::errno::ErrMemExceedThreshold, 8001);
}

#[test]
/// 多动作按优先级插入 fallback 链。
fn TestOOMActionPriority() {
    let tracker = NewTracker(1, 1);
    tracker.SetActionOnExceed(None);
    let mut states = Vec::new();
    for priority in [17, 4, 99, 0, 51, 73, 22, 88, 35, 61] {
        let (action, state) = MockAction::new(priority);
        states.push((priority, state));
        tracker.FallbackOldAndSetNewAction(Some(action));
    }
    states.sort_by_key(|(priority, _)| *priority);
    for expected in (0..states.len()).rev() {
        tracker.Consume(100);
        for (index, (_, state)) in states.iter().enumerate() {
            assert_eq!(state.calls.load(Ordering::SeqCst) > 0, index >= expected);
        }
    }
}

#[test]
/// 解绑 fallback 链中间节点后，其余动作保持原优先级顺序且被解绑动作不再触发。
fn unbind_middle_hard_limit_action_preserves_fallback_chain() {
    let tracker = NewTracker(1, 1);
    tracker.SetActionOnExceed(None);

    let (low, low_state) = MockAction::new(10);
    let (middle, middle_state) = MockAction::new(20);
    let middle_ptr = &*middle as *const dyn ActionOnExceed;
    let (high, high_state) = MockAction::new(30);
    tracker.FallbackOldAndSetNewAction(Some(low));
    tracker.FallbackOldAndSetNewAction(Some(middle));
    tracker.FallbackOldAndSetNewAction(Some(high));

    tracker.UnbindActionFromHardLimit(middle_ptr);
    tracker.Consume(1);
    tracker.Consume(1);

    assert_eq!(high_state.calls.load(Ordering::SeqCst), 2);
    assert_eq!(middle_state.calls.load(Ordering::SeqCst), 0);
    assert_eq!(low_state.calls.load(Ordering::SeqCst), 1);
}

#[test]
/// 全局内存仲裁器 limit/soft/workmode。
fn TestGlobalMemArbitrator() {
    let _guard = crate::global_arbitrator::GLOBAL_TEST_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    SetupGlobalMemArbitratorForTest(directory.path().display().to_string());

    let missing = directory.path().join("missing");
    let recorder = RuntimeMemStateRecorder::new(&missing);
    assert_eq!(
        recorder.load().unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    let state = serde_json::json!({"Version": 1, "LastRisk": {"HeapAlloc": 2, "QuotaAlloc": 3}, "Magnif": 4, "PoolMediumCap": 5});
    recorder.store(&state).unwrap();
    assert_eq!(recorder.load().unwrap(), Some(state));
    std::fs::write(missing.join("mem-state.v1.json"), b"??????").unwrap();
    assert!(recorder.load().is_err());

    assert_eq!(parse_soft_limit("0"), (0, 0.0, SoftLimitMode::Disable));
    assert_eq!(parse_soft_limit("auto"), (0, 0.0, SoftLimitMode::Auto));
    assert_eq!(
        parse_soft_limit("1024"),
        (1024, 0.0, SoftLimitMode::Specified)
    );
    assert_eq!(
        parse_soft_limit("0.88"),
        (0, 0.88, SoftLimitMode::Specified)
    );
    assert_eq!(parse_soft_limit("1.01"), (0, 0.0, SoftLimitMode::Disable));
    assert_eq!(
        parse_soft_limit("invalid"),
        (0, 0.0, SoftLimitMode::Disable)
    );

    SetGlobalMemArbitratorLimit(10_i64 << 30);
    SetGlobalMemArbitratorSoftLimit("0.88".to_owned());
    assert!(GlobalMemArbitrator().is_none());
    assert!(SetGlobalMemArbitratorWorkMode("standard".to_owned()));
    let arbitrator = GlobalMemArbitrator().unwrap();
    assert_eq!(arbitrator.WorkMode(), WorkMode::Standard);
    assert_eq!(arbitrator.Limit(), 10_i64 << 30);
    assert_eq!(
        arbitrator.SoftLimitConfig(),
        (0, 0.88, SoftLimitMode::Specified)
    );
    arbitrator.AddRootPool(719);
    arbitrator.AddRootPool(719);
    assert_eq!(arbitrator.RootPoolCount(), 1);
    assert!(RemovePoolFromGlobalMemArbitrator(719));
    assert_eq!(arbitrator.RootPoolCount(), 0);
    assert!(!RemovePoolFromGlobalMemArbitrator(719));
    assert!(SetGlobalMemArbitratorWorkMode("priority".to_owned()));
    assert_eq!(arbitrator.WorkMode(), WorkMode::Priority);
    assert!(SetGlobalMemArbitratorWorkMode("disable".to_owned()));
    assert!(GlobalMemArbitrator().is_none());
    CleanupGlobalMemArbitratorForTest();

    // Setup must isolate repeated test runs from all previous global state.
    SetupGlobalMemArbitratorForTest(directory.path().display().to_string());
    assert!(GlobalMemArbitrator().is_none());
    assert!(SetGlobalMemArbitratorWorkMode("standard".to_owned()));
    let arbitrator = GlobalMemArbitrator().unwrap();
    assert_eq!(arbitrator.Limit(), DefMaxLimit);
    assert_eq!(
        arbitrator.SoftLimitConfig(),
        (0, 0.0, SoftLimitMode::Disable)
    );
    CleanupGlobalMemArbitratorForTest();
}
