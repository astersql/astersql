// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Timer 运行时测试公共夹具。
//
// 提供等待辅助、可脚本化 Hook、全局 Hook 工厂以及脚本化 `TimerStore`，
// 供 runtime/worker 等测试复用。

use crate::api::{
    Cond, Context, Hook, HookFactory, PreSchedEventResult, TimerClient, TimerError, TimerRecord,
    TimerResult, TimerShedEvent, TimerStore, TimerStoreCore, TimerUpdate, WatchTimerChan,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// 在超时前轮询直到条件为真，否则断言失败。
pub fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "condition timed out after {timeout:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

/// 在给定时间窗口内断言条件始终为假。
pub fn check_not_done(after: Duration, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + after;
    while Instant::now() < deadline {
        assert!(!condition(), "operation completed unexpectedly");
        thread::sleep(Duration::from_millis(5));
    }
}

/// 串行化依赖全局状态的测试。
static SERIAL: Mutex<()> = Mutex::new(());

/// 获取全局串行锁。
pub fn serial_guard() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone)]
/// 可脚本化的测试 Hook：记录调用并可预设返回值/阻塞/panic。
pub struct TestHook {
    inner: Arc<TestHookInner>,
}

/// TestHook 的共享内部状态。
struct TestHookInner {
    starts: AtomicUsize,
    stops: AtomicUsize,
    pre_calls: Mutex<Vec<TimerRecord>>,
    sched_calls: Mutex<Vec<TimerRecord>>,
    pre_results: Mutex<VecDeque<TimerResult<PreSchedEventResult>>>,
    sched_results: Mutex<VecDeque<TimerResult<()>>>,
    pre_block: Mutex<Option<Duration>>,
    pre_panic: AtomicBool,
}

/// TestHook 构造与观测 API。
impl TestHook {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(TestHookInner {
                starts: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
                pre_calls: Mutex::new(Vec::new()),
                sched_calls: Mutex::new(Vec::new()),
                pre_results: Mutex::new(VecDeque::new()),
                sched_results: Mutex::new(VecDeque::new()),
                pre_block: Mutex::new(None),
                pre_panic: AtomicBool::new(false),
            }),
        }
    }

    /// 预设下一次 OnPreSchedEvent 返回值。
    pub fn push_pre_result(&self, result: TimerResult<PreSchedEventResult>) {
        self.inner.pre_results.lock().unwrap().push_back(result);
    }

    /// 预设下一次 OnSchedEvent 返回值。
    pub fn push_sched_result(&self, result: TimerResult<()>) {
        self.inner.sched_results.lock().unwrap().push_back(result);
    }

    /// 让下一次 PreSched 阻塞指定时长。
    pub fn set_pre_block(&self, duration: Duration) {
        *self.inner.pre_block.lock().unwrap() = Some(duration);
    }

    /// 让下一次 PreSched panic。
    pub fn set_pre_panic(&self) {
        self.inner.pre_panic.store(true, Ordering::SeqCst);
    }

    /// Start 调用次数。
    pub fn starts(&self) -> usize {
        self.inner.starts.load(Ordering::SeqCst)
    }

    /// Stop 调用次数。
    pub fn stops(&self) -> usize {
        self.inner.stops.load(Ordering::SeqCst)
    }

    /// 已观察到的 PreSched 定时器快照。
    pub fn pre_calls(&self) -> Vec<TimerRecord> {
        self.inner.pre_calls.lock().unwrap().clone()
    }

    /// 已观察到的 OnSched 定时器快照。
    pub fn sched_calls(&self) -> Vec<TimerRecord> {
        self.inner.sched_calls.lock().unwrap().clone()
    }
}

/// 实现 Hook：按脚本返回结果。
impl Hook for TestHook {
    fn Start(&mut self) {
        self.inner.starts.fetch_add(1, Ordering::SeqCst);
    }

    fn Stop(&mut self) {
        self.inner.stops.fetch_add(1, Ordering::SeqCst);
    }

    fn OnPreSchedEvent(
        &mut self,
        _ctx: &Context,
        event: &dyn TimerShedEvent,
    ) -> TimerResult<PreSchedEventResult> {
        if let Some(duration) = self.inner.pre_block.lock().unwrap().take() {
            thread::sleep(duration);
        }
        if self.inner.pre_panic.swap(false, Ordering::SeqCst) {
            panic!("test hook pre-schedule panic");
        }
        self.inner
            .pre_calls
            .lock()
            .unwrap()
            .push(event.Timer().expect("event contains timer"));
        self.inner
            .pre_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(PreSchedEventResult::default()))
    }

    fn OnSchedEvent(&mut self, _ctx: &Context, event: &dyn TimerShedEvent) -> TimerResult<()> {
        self.inner
            .sched_calls
            .lock()
            .unwrap()
            .push(event.Timer().expect("event contains timer"));
        self.inner
            .sched_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(()))
    }
}

#[derive(Default)]
/// 全局 Hook 工厂状态：当前钩子与已请求的 class 列表。
struct FactoryState {
    hook: Option<TestHook>,
    classes: Vec<String>,
}

/// 取得进程内共享的工厂状态。
fn factory_state() -> &'static Mutex<FactoryState> {
    static STATE: OnceLock<Mutex<FactoryState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(FactoryState::default()))
}

/// 安装测试 Hook 并返回可注册到运行时的工厂。
pub fn install_hook(hook: TestHook) -> HookFactory {
    let mut state = factory_state().lock().unwrap();
    state.hook = Some(hook);
    state.classes.clear();
    Arc::new(test_hook_factory)
}

/// 返回工厂被调用过的 HookClass 列表。
pub fn factory_classes() -> Vec<String> {
    factory_state().lock().unwrap().classes.clone()
}

/// 工厂实现：记录 class 并克隆已安装的 TestHook。
fn test_hook_factory(hook_class: String, _client: Box<dyn TimerClient>) -> Box<dyn Hook> {
    let mut state = factory_state().lock().unwrap();
    state.classes.push(hook_class);
    Box::new(state.hook.as_ref().expect("test hook installed").clone())
}

#[derive(Clone)]
/// 可脚本化的 `TimerStoreCore`：按队列返回 List/Watch 结果。
pub struct ScriptedStoreCore {
    inner: Arc<ScriptedStoreInner>,
}

/// List 脚本动作：返回结果或 panic。
enum ListAction {
    Return(TimerResult<Vec<TimerRecord>>),
    Panic(String),
}

/// ScriptedStore 内部队列与调用计数。
struct ScriptedStoreInner {
    lists: Mutex<VecDeque<ListAction>>,
    watches: Mutex<VecDeque<WatchTimerChan>>,
    watch_supported: Mutex<VecDeque<bool>>,
    list_calls: AtomicUsize,
    watch_calls: AtomicUsize,
    observed_ids: Mutex<Vec<Vec<String>>>,
}

/// 脚本注入与观测 API。
impl ScriptedStoreCore {
    fn new() -> Self {
        Self {
            inner: Arc::new(ScriptedStoreInner {
                lists: Mutex::new(VecDeque::new()),
                watches: Mutex::new(VecDeque::new()),
                watch_supported: Mutex::new(VecDeque::new()),
                list_calls: AtomicUsize::new(0),
                watch_calls: AtomicUsize::new(0),
                observed_ids: Mutex::new(Vec::new()),
            }),
        }
    }

    /// 入队一次成功的 List 结果。
    pub fn push_list(&self, timers: Vec<TimerRecord>) {
        self.inner
            .lists
            .lock()
            .unwrap()
            .push_back(ListAction::Return(Ok(timers)));
    }

    /// 入队一次 List 错误。
    pub fn push_list_error(&self, message: &str) {
        self.inner
            .lists
            .lock()
            .unwrap()
            .push_back(ListAction::Return(Err(TimerError::message(message))));
    }

    /// 入队一次 List panic。
    pub fn push_list_panic(&self, message: &str) {
        self.inner
            .lists
            .lock()
            .unwrap()
            .push_back(ListAction::Panic(message.to_string()));
    }

    /// 入队下一次 WatchSupported 返回值。
    pub fn push_watch_supported(&self, supported: bool) {
        self.inner
            .watch_supported
            .lock()
            .unwrap()
            .push_back(supported);
    }

    /// 入队下一次 Watch 通道。
    pub fn push_watch(&self, watch: WatchTimerChan) {
        self.inner.watches.lock().unwrap().push_back(watch);
    }

    /// List 调用次数。
    pub fn list_calls(&self) -> usize {
        self.inner.list_calls.load(Ordering::SeqCst)
    }

    /// Watch 调用次数。
    pub fn watch_calls(&self) -> usize {
        self.inner.watch_calls.load(Ordering::SeqCst)
    }

    /// 每次 List 条件中观察到的 ID 集合。
    pub fn observed_ids(&self) -> Vec<Vec<String>> {
        self.inner.observed_ids.lock().unwrap().clone()
    }
}

/// 创建脚本化存储及其 `TimerStore` 包装。
pub fn new_mock_store() -> (ScriptedStoreCore, TimerStore) {
    let core = ScriptedStoreCore::new();
    (core.clone(), TimerStore::from_core(core))
}

/// 从条件树中递归收集 Timer ID（用于观测查询条件）。
fn collect_ids(cond: &dyn Cond, ids: &mut Vec<String>) {
    if let Some(timer_cond) = cond.as_any().downcast_ref::<crate::api::TimerCond>() {
        if let Some(id) = timer_cond.ID.Get() {
            ids.push(id.clone());
        }
        return;
    }
    if let Some(operator) = cond.as_any().downcast_ref::<crate::api::Operator>() {
        for child in &operator.Children {
            collect_ids(child.as_ref(), ids);
        }
    }
}

/// 仅实现测试需要的 List/Watch；写操作一律报错。
impl TimerStoreCore for ScriptedStoreCore {
    fn Create(&self, _ctx: &Context, _record: Option<TimerRecord>) -> TimerResult<String> {
        Err(TimerError::message("unexpected Create"))
    }

    fn List(&self, _ctx: &Context, cond: Option<&dyn Cond>) -> TimerResult<Vec<TimerRecord>> {
        self.inner.list_calls.fetch_add(1, Ordering::SeqCst);
        let mut ids = Vec::new();
        if let Some(cond) = cond {
            collect_ids(cond, &mut ids);
        }
        ids.sort();
        self.inner.observed_ids.lock().unwrap().push(ids);
        let action = self
            .inner
            .lists
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front();
        match action {
            Some(ListAction::Return(result)) => result,
            Some(ListAction::Panic(message)) => panic!("{message}"),
            None => Ok(Vec::new()),
        }
    }

    fn Update(
        &self,
        _ctx: &Context,
        _timer_id: &str,
        _update: Option<TimerUpdate>,
    ) -> TimerResult<()> {
        Err(TimerError::message("unexpected Update"))
    }

    fn Delete(&self, _ctx: &Context, _timer_id: &str) -> TimerResult<bool> {
        Err(TimerError::message("unexpected Delete"))
    }

    fn WatchSupported(&self) -> bool {
        self.inner
            .watch_supported
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(false)
    }

    fn Watch(&self, _ctx: &Context) -> WatchTimerChan {
        self.inner.watch_calls.fetch_add(1, Ordering::SeqCst);
        self.inner
            .watches
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(crossbeam_channel::never)
    }

    fn Close(&self) {}
}
