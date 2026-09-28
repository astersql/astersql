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

// Hook Worker 单元测试。
//
// 覆盖 Idle/Trigger 成功路径、PreSched 延迟与错误、手动请求超时，
// 以及 Worker 循环对 panic 的恢复。

#![allow(non_snake_case)]

use astersql_timer_runtime::api::{
    Cond, Context, ErrTimerNotExist, ErrVersionNotMatch, EventExtra, Hook, ManualRequest,
    NewDefaultTimerClient, NewMemoryTimerStore, PreSchedEventResult, SchedEventIdle,
    SchedEventInterval, SchedEventTrigger, TimerClient, TimerError, TimerRecord, TimerResult,
    TimerShedEvent, TimerSpec, TimerStore, TimerStoreCore, TimerUpdate, WatchTimerChan,
    WithSetSchedExpr, WithSetSummaryData, WithSetWatermark,
};
use astersql_timer_runtime::worker::{
    HookFactoryFn, HookWorker, TriggerEventRequest, TriggerEventResponse, newHookWorker,
    newHookWorkerWithRetry, workerEventDefaultRetryInterval,
};
use chrono::{Duration as ChronoDuration, Utc};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 返回当前固定偏移时间戳。
fn now() -> astersql_timer_runtime::api::Timestamp {
    Utc::now().fixed_offset()
}

/// 轮询等待谓词成立，超时则 panic。
fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for worker state"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// 通过客户端创建并返回一个可用的测试定时器。
fn prepareTimer(client: &impl TimerClient) -> TimerRecord {
    let ctx = Context::todo();
    let started = now();
    let mut timer = client
        .CreateTimer(
            &ctx,
            TimerSpec {
                Key: "key1".to_string(),
                Data: b"data1".to_vec(),
                SchedPolicyType: SchedEventInterval.to_string(),
                SchedPolicyExpr: "1m".to_string(),
                HookClass: "h1".to_string(),
                Enable: true,
                ..TimerSpec::default()
            },
        )
        .unwrap();
    let watermark = started - ChronoDuration::minutes(1);
    client
        .UpdateTimer(
            &ctx,
            &timer.ID,
            vec![
                WithSetWatermark(watermark),
                WithSetSummaryData(b"summary1".to_vec()),
            ],
        )
        .unwrap();
    timer = client.GetTimerByID(&ctx, &timer.ID).unwrap();

    assert!(!timer.ID.is_empty());
    assert_eq!(timer.Data, b"data1");
    assert_eq!(timer.SchedPolicyType, SchedEventInterval);
    assert_eq!(timer.SchedPolicyExpr, "1m");
    assert_eq!(timer.HookClass, "h1");
    assert!(timer.Enable);
    assert_eq!(timer.Watermark.unwrap().timestamp(), watermark.timestamp());
    assert_eq!(timer.SummaryData, b"summary1");
    assert!(timer.CreateTime.unwrap() >= started);
    assert!(timer.CreateTime.unwrap() <= now());
    assert!(timer.Version > 0);
    assert!(timer.EventID.is_empty());
    assert_eq!(timer.EventStatus, SchedEventIdle);
    assert!(timer.EventData.is_empty());
    assert!(timer.EventStart.is_none());
    timer
}

/// 构造指向指定响应通道的触发请求。
fn request(
    timer: TimerRecord,
    eventID: &str,
    store: TimerStore,
) -> (
    TriggerEventRequest,
    crossbeam_channel::Receiver<TriggerEventResponse>,
) {
    let (tx, rx) = crossbeam_channel::bounded(1);
    (
        TriggerEventRequest {
            eventID: eventID.to_string(),
            timer,
            store,
            resp: tx,
        },
        rx,
    )
}

/// 投递请求并断言收到期望的响应字段。
fn sendWorkerRequestAndCheckResp(
    worker: &HookWorker,
    request: TriggerEventRequest,
    receiver: &crossbeam_channel::Receiver<TriggerEventResponse>,
) -> TriggerEventResponse {
    worker
        .ch
        .send_timeout(request, Duration::from_secs(1))
        .expect("worker request channel should be writable");
    let response = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("worker should respond");
    assert!(receiver.try_recv().is_err());
    response
}

/// 取消上下文并 join Worker。
fn stop(worker: &HookWorker, cancel: &astersql_timer_runtime::api::CancelContext) {
    cancel.cancel();
    worker.stopAndJoin();
}

/// 断言 Worker 六项计数器与期望一致。
fn checkWorkerCounterValues(worker: &HookWorker, expected: [u64; 6]) {
    let counters = &worker.counters;
    assert_eq!(
        [
            counters.triggerRequest.load(Ordering::Relaxed),
            counters.onPreSchedEvent.load(Ordering::Relaxed),
            counters.onPreSchedEventErr.load(Ordering::Relaxed),
            counters.onPreSchedEventDelay.load(Ordering::Relaxed),
            counters.onSchedEvent.load(Ordering::Relaxed),
            counters.onSchedEventErr.load(Ordering::Relaxed),
        ],
        expected
    );
}

/// 脚本化 PreSched 动作：成功/延迟/错误。
enum PreAction {
    Return(TimerResult<PreSchedEventResult>),
    Panic,
}

/// 脚本化 OnSched 动作：成功/错误。
enum SchedAction {
    Return(TimerResult<()>),
    Panic,
}

#[derive(Default)]
/// MockHook 共享状态：动作队列与调用记录。
struct HookState {
    starts: AtomicU64,
    stops: AtomicU64,
    preCalls: AtomicU64,
    schedCalls: AtomicU64,
    startPanic: AtomicBool,
    stopPanic: AtomicBool,
    preActions: Mutex<VecDeque<PreAction>>,
    schedActions: Mutex<VecDeque<SchedAction>>,
    events: Mutex<Vec<(String, TimerRecord)>>,
}

/// HookState 辅助方法。
impl HookState {
    fn pre(&self, action: PreAction) {
        self.preActions.lock().unwrap().push_back(action);
    }

    fn sched(&self, action: SchedAction) {
        self.schedActions.lock().unwrap().push_back(action);
    }
}

/// 基于共享状态的可脚本 Hook。
struct MockHook(Arc<HookState>);

/// 按队列弹出动作执行回调。
impl Hook for MockHook {
    fn Start(&mut self) {
        self.0.starts.fetch_add(1, Ordering::Relaxed);
        assert!(
            !self.0.startPanic.swap(false, Ordering::Relaxed),
            "hook start panic"
        );
    }

    fn Stop(&mut self) {
        self.0.stops.fetch_add(1, Ordering::Relaxed);
        assert!(
            !self.0.stopPanic.swap(false, Ordering::Relaxed),
            "hook stop panic"
        );
    }

    fn OnPreSchedEvent(
        &mut self,
        _ctx: &Context,
        event: &dyn TimerShedEvent,
    ) -> TimerResult<PreSchedEventResult> {
        self.0.preCalls.fetch_add(1, Ordering::Relaxed);
        self.0
            .events
            .lock()
            .unwrap()
            .push((event.EventID(), event.Timer().unwrap()));
        let action = self.0.preActions.lock().unwrap().pop_front();
        match action {
            Some(PreAction::Return(result)) => result,
            Some(PreAction::Panic) => panic!("OnPreSchedEvent panic"),
            None => Ok(PreSchedEventResult::default()),
        }
    }

    fn OnSchedEvent(&mut self, _ctx: &Context, event: &dyn TimerShedEvent) -> TimerResult<()> {
        self.0.schedCalls.fetch_add(1, Ordering::Relaxed);
        self.0
            .events
            .lock()
            .unwrap()
            .push((event.EventID(), event.Timer().unwrap()));
        let action = self.0.schedActions.lock().unwrap().pop_front();
        match action {
            Some(SchedAction::Return(result)) => result,
            Some(SchedAction::Panic) => panic!("OnSchedEvent panic"),
            None => Ok(()),
        }
    }
}

/// 固定返回绑定给定状态的 MockHook 工厂。
fn factory(state: Arc<HookState>) -> HookFactoryFn {
    Arc::new(move || Box::new(MockHook(Arc::clone(&state))))
}

/// 工厂脚本：返回 Hook 或 panic。
enum FactoryAction {
    Hook(Arc<HookState>),
    Panic,
}

/// 按动作队列创建 Hook 的工厂。
fn scriptedFactory(actions: Vec<FactoryAction>) -> HookFactoryFn {
    let actions = Arc::new(Mutex::new(VecDeque::from(actions)));
    Arc::new(move || {
        let action = actions.lock().unwrap().pop_front();
        match action {
            Some(FactoryAction::Hook(state)) => Box::new(MockHook(state)),
            Some(FactoryAction::Panic) => panic!("hook factory panic"),
            None => panic!("unexpected hook factory call"),
        }
    })
}

#[derive(Default)]
/// 脚本化存储的 Get/Update 队列状态。
struct StoreState {
    updates: Mutex<VecDeque<TimerResult<()>>>,
    lists: Mutex<VecDeque<TimerResult<Vec<TimerRecord>>>>,
    updateCalls: AtomicU64,
    listCalls: AtomicU64,
}

#[derive(Clone)]
/// 仅实现触发路径需要的 GetByID/Update。
struct ScriptedStore(Arc<StoreState>);

/// 按队列返回 Get/Update 结果。
impl TimerStoreCore for ScriptedStore {
    fn Create(&self, _ctx: &Context, _record: Option<TimerRecord>) -> TimerResult<String> {
        Err(TimerError::message("unexpected Create"))
    }

    fn List(&self, _ctx: &Context, _cond: Option<&dyn Cond>) -> TimerResult<Vec<TimerRecord>> {
        self.0.listCalls.fetch_add(1, Ordering::Relaxed);
        self.0
            .lists
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(TimerError::message("unexpected List")))
    }

    fn Update(
        &self,
        _ctx: &Context,
        _timerID: &str,
        _update: Option<TimerUpdate>,
    ) -> TimerResult<()> {
        self.0.updateCalls.fetch_add(1, Ordering::Relaxed);
        self.0
            .updates
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(TimerError::message("unexpected Update")))
    }

    fn Delete(&self, _ctx: &Context, _timerID: &str) -> TimerResult<bool> {
        Err(TimerError::message("unexpected Delete"))
    }

    fn WatchSupported(&self) -> bool {
        false
    }

    fn Watch(&self, _ctx: &Context) -> WatchTimerChan {
        crossbeam_channel::never()
    }

    fn Close(&self) {}
}

/// 构造带预设 Get/Update 队列的 `TimerStore`。
fn scriptedStore(
    updates: Vec<TimerResult<()>>,
    lists: Vec<TimerResult<Vec<TimerRecord>>>,
) -> (Arc<StoreState>, TimerStore) {
    let state = Arc::new(StoreState {
        updates: Mutex::new(VecDeque::from(updates)),
        lists: Mutex::new(VecDeque::from(lists)),
        ..StoreState::default()
    });
    let store = TimerStore::from_core(ScriptedStore(Arc::clone(&state)));
    (state, store)
}

/// 断言响应为失败且带默认重试间隔。
fn assertRetry(response: &TriggerEventResponse, timerID: &str, eventID: &str) {
    assert!(!response.success);
    assert_eq!(response.timerID, timerID);
    assert_eq!(response.eventID, eventID);
    assert_eq!(
        response.retryAfter.Get(),
        Some(&workerEventDefaultRetryInterval)
    );
    assert!(!response.newTimerRecord.Present());
}

#[test]
/// 验证 Worker Start/Stop 与 Hook 生命周期。
fn TestWorkerStartStop() {
    let (ctx, cancel) = Context::with_cancel();
    let hook = Arc::new(HookState::default());
    let worker = newHookWorker(
        ctx,
        "g1".to_string(),
        "h1".to_string(),
        Some(factory(Arc::clone(&hook))),
        None,
    );
    wait_until(Duration::from_secs(1), || {
        hook.starts.load(Ordering::Relaxed) == 1
    });
    assert_eq!(hook.stops.load(Ordering::Relaxed), 0);
    stop(&worker, &cancel);
    assert_eq!(hook.stops.load(Ordering::Relaxed), 1);
}

#[test]
/// Idle 定时器完整 PreSched + 落库 + OnSched 成功路径。
fn TestWorkerProcessIdleTimerSuccess() {
    let store = NewMemoryTimerStore();
    let client = NewDefaultTimerClient(store.clone());
    let timer = prepareTimer(&client);
    let eventID = uuid::Uuid::new_v4().simple().to_string();
    let hook = Arc::new(HookState::default());
    hook.pre(PreAction::Return(Ok(PreSchedEventResult {
        EventData: b"eventdata".to_vec(),
        ..PreSchedEventResult::default()
    })));
    hook.sched(SchedAction::Return(Ok(())));
    let (ctx, cancel) = Context::with_cancel();
    let worker = newHookWorker(
        ctx.clone(),
        "g1".to_string(),
        "h1".to_string(),
        Some(factory(Arc::clone(&hook))),
        None,
    );
    let (request, receiver) = request(timer.clone(), &eventID, store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, request, &receiver);

    assert!(response.success);
    assert_eq!(response.timerID, timer.ID);
    assert_eq!(response.eventID, eventID);
    assert!(!response.retryAfter.Present());
    let triggered = response.newTimerRecord.Get().unwrap().as_ref().unwrap();
    assert_eq!(triggered, &store.GetByID(&ctx, &timer.ID).unwrap());
    assert_eq!(triggered.EventID, eventID);
    assert_eq!(triggered.EventData, b"eventdata");
    assert_eq!(triggered.EventStatus, SchedEventTrigger);
    assert_eq!(triggered.EventExtra.EventWatermark, timer.Watermark);
    assert!(triggered.EventStart.is_some());
    assert!(triggered.Version > timer.Version);
    assert_eq!(hook.events.lock().unwrap()[0], (eventID.clone(), timer));
    checkWorkerCounterValues(&worker, [1, 1, 0, 0, 1, 0]);
    stop(&worker, &cancel);
    store.Close();
}

#[test]
/// 已处于 Trigger 状态时跳过 PreSched，仅跑 OnSched。
fn TestWorkerProcessTriggeredTimerSuccess() {
    let store = NewMemoryTimerStore();
    let client = NewDefaultTimerClient(store.clone());
    let timer = prepareTimer(&client);
    let eventID = uuid::Uuid::new_v4().simple().to_string();
    let eventStart = now();
    let mut update = TimerUpdate::default();
    update.EventID.Set(eventID.clone());
    update.EventStatus.Set(SchedEventTrigger.to_string());
    update.EventData.Set(b"eventdata".to_vec());
    update.EventStart.Set(Some(eventStart));
    update.EventExtra.Set(EventExtra {
        EventWatermark: timer.Watermark,
        ..EventExtra::default()
    });
    store
        .Update(&Context::todo(), &timer.ID, Some(update))
        .unwrap();
    let triggered = store.GetByID(&Context::todo(), &timer.ID).unwrap();

    let hook = Arc::new(HookState::default());
    hook.sched(SchedAction::Return(Ok(())));
    let (ctx, cancel) = Context::with_cancel();
    let worker = newHookWorker(
        ctx,
        "g1".to_string(),
        "h1".to_string(),
        Some(factory(Arc::clone(&hook))),
        None,
    );
    let (request, receiver) = request(triggered.clone(), &eventID, store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, request, &receiver);
    assert!(response.success);
    assert_eq!(response.newTimerRecord.Get(), Some(&Some(triggered)));
    assert!(!response.retryAfter.Present());
    checkWorkerCounterValues(&worker, [1, 0, 0, 0, 1, 0]);
    stop(&worker, &cancel);
    store.Close();
}

#[test]
/// PreSched 延迟/错误与 OnSched 错误的重试行为。
fn TestWorkerProcessDelayOrErr() {
    let store = NewMemoryTimerStore();
    let client = NewDefaultTimerClient(store.clone());
    let original = prepareTimer(&client);
    let eventID = uuid::Uuid::new_v4().simple().to_string();
    let hook = Arc::new(HookState::default());
    let (ctx, cancel) = Context::with_cancel();
    let worker = newHookWorker(
        ctx.clone(),
        "g1".to_string(),
        "h1".to_string(),
        Some(factory(Arc::clone(&hook))),
        None,
    );

    hook.pre(PreAction::Return(Ok(PreSchedEventResult {
        Delay: Duration::from_secs(5),
        ..PreSchedEventResult::default()
    })));
    let (req, rx) = request(original.clone(), &eventID, store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert_eq!(response.retryAfter.Get(), Some(&Duration::from_secs(5)));
    assert!(!response.success);
    assert!(!response.newTimerRecord.Present());
    checkWorkerCounterValues(&worker, [1, 1, 0, 1, 0, 0]);

    hook.pre(PreAction::Return(Err(TimerError::message("mockErr"))));
    let (req, rx) = request(original.clone(), &eventID, store.clone());
    assertRetry(
        &sendWorkerRequestAndCheckResp(&worker, req, &rx),
        &original.ID,
        &eventID,
    );
    checkWorkerCounterValues(&worker, [2, 2, 1, 1, 0, 0]);
    assert_eq!(client.GetTimerByID(&ctx, &original.ID).unwrap(), original);

    for (updateResult, listResult, expectedNew) in [
        (Err(TimerError::message("mockErr")), None, None),
        (
            Err(ErrVersionNotMatch),
            Some(Err(TimerError::message("mockErr"))),
            None,
        ),
        (Ok(()), Some(Err(TimerError::message("mockErr"))), None),
    ] {
        hook.pre(PreAction::Return(Ok(PreSchedEventResult::default())));
        let lists = listResult.into_iter().collect();
        let (_, mockStore) = scriptedStore(vec![updateResult], lists);
        let (req, rx) = request(original.clone(), &eventID, mockStore);
        let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
        assertRetry(&response, &original.ID, &eventID);
        assert_eq!(response.newTimerRecord.Get(), expectedNew.as_ref());
    }

    // Go reuses the GetByID error after an update version conflict. If the
    // record disappeared, the response carries an explicit deleted marker and
    // retries immediately.
    hook.pre(PreAction::Return(Ok(PreSchedEventResult::default())));
    let (_, mockStore) = scriptedStore(vec![Err(ErrVersionNotMatch)], vec![Err(ErrTimerNotExist)]);
    let (req, rx) = request(original.clone(), &eventID, mockStore);
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert!(!response.retryAfter.Present());
    assert_eq!(response.newTimerRecord.Get(), Some(&None));

    hook.pre(PreAction::Return(Ok(PreSchedEventResult::default())));
    let (_, mockStore) = scriptedStore(vec![Ok(())], vec![Ok(Vec::new())]);
    let (req, rx) = request(original.clone(), &eventID, mockStore);
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert!(!response.retryAfter.Present());
    assert_eq!(response.newTimerRecord.Get(), Some(&None));

    let mut other = original.clone();
    other.Version += 2;
    other.EventStatus = SchedEventTrigger.to_string();
    other.EventID = "anothereventid".to_string();
    other.EventStart = Some(now());
    hook.pre(PreAction::Return(Ok(PreSchedEventResult::default())));
    let (_, mockStore) = scriptedStore(vec![Ok(())], vec![Ok(vec![other.clone()])]);
    let (req, rx) = request(original.clone(), &eventID, mockStore);
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert!(!response.retryAfter.Present());
    assert_eq!(response.newTimerRecord.Get(), Some(&Some(other)));
    checkWorkerCounterValues(&worker, [8, 8, 1, 1, 0, 0]);

    client
        .UpdateTimer(
            &ctx,
            &original.ID,
            vec![WithSetSchedExpr(
                SchedEventInterval.to_string(),
                "2m".to_string(),
            )],
        )
        .unwrap();
    let current = client.GetTimerByID(&ctx, &original.ID).unwrap();
    hook.pre(PreAction::Return(Ok(PreSchedEventResult::default())));
    let (req, rx) = request(original.clone(), &eventID, store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert!(!response.retryAfter.Present());
    assert_eq!(response.newTimerRecord.Get(), Some(&Some(current.clone())));
    checkWorkerCounterValues(&worker, [9, 9, 1, 1, 0, 0]);

    hook.pre(PreAction::Return(Ok(PreSchedEventResult {
        EventData: b"eventdata".to_vec(),
        ..PreSchedEventResult::default()
    })));
    hook.sched(SchedAction::Return(Err(TimerError::message("mockErr"))));
    let (req, rx) = request(current.clone(), &eventID, store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert_eq!(
        response.retryAfter.Get(),
        Some(&workerEventDefaultRetryInterval)
    );
    let triggered = client.GetTimerByID(&ctx, &current.ID).unwrap();
    assert_eq!(
        response.newTimerRecord.Get(),
        Some(&Some(triggered.clone()))
    );
    checkWorkerCounterValues(&worker, [10, 10, 1, 1, 1, 1]);

    client
        .CloseTimerEvent(&ctx, &triggered.ID, &eventID, Vec::new())
        .unwrap();
    let (req, rx) = request(triggered.clone(), &eventID, store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    let closed = client.GetTimerByID(&ctx, &triggered.ID).unwrap();
    assert!(closed.EventID.is_empty());
    assert_eq!(response.newTimerRecord.Get(), Some(&Some(closed)));
    assert!(!response.retryAfter.Present());

    assert!(client.DeleteTimer(&ctx, &triggered.ID).unwrap());
    let (req, rx) = request(triggered, &eventID, store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert_eq!(response.newTimerRecord.Get(), Some(&None));
    assert!(!response.retryAfter.Present());

    let deleted = prepareTimer(&client);
    assert!(client.DeleteTimer(&ctx, &deleted.ID).unwrap());
    hook.pre(PreAction::Return(Ok(PreSchedEventResult {
        EventData: b"eventdata".to_vec(),
        ..PreSchedEventResult::default()
    })));
    let deletedEventID = uuid::Uuid::new_v4().simple().to_string();
    let (req, rx) = request(deleted.clone(), &deletedEventID, store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert_eq!(response.newTimerRecord.Get(), Some(&None));
    assert!(!response.retryAfter.Present());

    stop(&worker, &cancel);
    store.Close();
}

#[test]
/// 手动触发超时与正常处理路径。
fn TestWorkerProcessManualRequest() {
    let store = NewMemoryTimerStore();
    let client = NewDefaultTimerClient(store.clone());
    let mut timer = prepareTimer(&client);
    let ctx = Context::background();
    let mut update = TimerUpdate::default();
    update.ManualRequest.Set(ManualRequest {
        ManualRequestID: "req1".to_string(),
        ManualRequestTime: Some(now() - ChronoDuration::minutes(1)),
        ManualTimeout: Duration::from_secs(59),
        ..ManualRequest::default()
    });
    store.Update(&ctx, &timer.ID, Some(update)).unwrap();
    timer = client.GetTimerByID(&ctx, &timer.ID).unwrap();

    let hook = Arc::new(HookState::default());
    let (workerCtx, cancel) = Context::with_cancel();
    let worker = newHookWorker(
        workerCtx,
        "g1".to_string(),
        "h1".to_string(),
        Some(factory(Arc::clone(&hook))),
        Some(Arc::new(now)),
    );

    let eventID = uuid::Uuid::new_v4().simple().to_string();
    let (_, mockStore) = scriptedStore(vec![Err(TimerError::message("mockErr"))], Vec::new());
    let (req, rx) = request(timer.clone(), &eventID, mockStore);
    assertRetry(
        &sendWorkerRequestAndCheckResp(&worker, req, &rx),
        &timer.ID,
        &eventID,
    );

    // Go keeps the request's timer snapshot when closing an expired manual
    // request races with deletion and Update reports ErrTimerNotExist.
    let deletedEventID = uuid::Uuid::new_v4().simple().to_string();
    let (_, mockStore) = scriptedStore(vec![Err(ErrTimerNotExist)], Vec::new());
    let (req, rx) = request(timer.clone(), &deletedEventID, mockStore);
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert!(!response.success);
    assert!(!response.retryAfter.Present());
    assert_eq!(response.newTimerRecord.Get(), Some(&Some(timer.clone())));

    let secondEventID = uuid::Uuid::new_v4().simple().to_string();
    let (_, mockStore) = scriptedStore(vec![Ok(())], vec![Err(TimerError::message("mockErr"))]);
    let (req, rx) = request(timer.clone(), &secondEventID, mockStore);
    assertRetry(
        &sendWorkerRequestAndCheckResp(&worker, req, &rx),
        &timer.ID,
        &secondEventID,
    );

    let thirdEventID = uuid::Uuid::new_v4().simple().to_string();
    let (req, rx) = request(timer.clone(), &thirdEventID, store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert!(!response.success);
    assert!(!response.retryAfter.Present());
    let processed = response.newTimerRecord.Get().unwrap().as_ref().unwrap();
    assert!(processed.ManualRequest.ManualProcessed);
    assert!(processed.ManualRequest.ManualEventID.is_empty());

    let requestID = client.ManualTriggerEvent(&ctx, &timer.ID).unwrap();
    timer = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    let finalEventID = uuid::Uuid::new_v4().simple().to_string();
    hook.pre(PreAction::Return(Ok(PreSchedEventResult::default())));
    hook.sched(SchedAction::Return(Ok(())));
    let (req, rx) = request(timer.clone(), &finalEventID, store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert!(response.success);
    assert!(!response.retryAfter.Present());
    let got = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert_eq!(response.newTimerRecord.Get(), Some(&Some(got.clone())));
    assert!(got.ManualRequest.ManualProcessed);
    assert_eq!(got.ManualRequest.ManualEventID, finalEventID);
    assert_eq!(got.EventID, finalEventID);
    assert_eq!(got.EventStatus, SchedEventTrigger);
    assert_eq!(
        got.EventExtra,
        EventExtra {
            EventManualRequestID: requestID,
            EventWatermark: timer.Watermark
        }
    );

    stop(&worker, &cancel);
    assert_eq!(hook.starts.load(Ordering::Relaxed), 1);
    assert_eq!(hook.stops.load(Ordering::Relaxed), 1);
    store.Close();
}

#[test]
/// 工厂/Start panic 后循环可恢复。
fn TestHookWorkerLoopPanicRecover() {
    let hook1 = Arc::new(HookState::default());
    hook1.startPanic.store(true, Ordering::Relaxed);
    let hook2 = Arc::new(HookState::default());
    hook2.startPanic.store(true, Ordering::Relaxed);
    hook2.stopPanic.store(true, Ordering::Relaxed);
    let hook3 = Arc::new(HookState::default());
    hook3.pre(PreAction::Return(Ok(PreSchedEventResult::default())));
    hook3.sched(SchedAction::Return(Ok(())));
    let hookFn = scriptedFactory(vec![
        FactoryAction::Panic,
        FactoryAction::Panic,
        FactoryAction::Hook(Arc::clone(&hook1)),
        FactoryAction::Panic,
        FactoryAction::Hook(Arc::clone(&hook2)),
        FactoryAction::Hook(Arc::clone(&hook3)),
    ]);
    let (ctx, cancel) = Context::with_cancel();
    let worker = newHookWorkerWithRetry(
        ctx.clone(),
        "g1".to_string(),
        "h1".to_string(),
        Some(hookFn),
        None,
        Duration::from_millis(1),
        Duration::from_secs(60),
    );
    wait_until(Duration::from_secs(2), || {
        hook3.starts.load(Ordering::Relaxed) == 1
    });

    let store = NewMemoryTimerStore();
    let client = NewDefaultTimerClient(store.clone());
    let timer = prepareTimer(&client);
    let eventID = "event1";
    let (req, rx) = request(timer.clone(), eventID, store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assert!(response.success);
    assert_eq!(response.timerID, timer.ID);
    assert_eq!(response.eventID, eventID);
    assert!(response.newTimerRecord.Present());
    assert_eq!(hook1.starts.load(Ordering::Relaxed), 1);
    assert_eq!(hook1.stops.load(Ordering::Relaxed), 1);
    assert_eq!(hook2.starts.load(Ordering::Relaxed), 1);
    assert_eq!(hook2.stops.load(Ordering::Relaxed), 1);

    hook3.stopPanic.store(true, Ordering::Relaxed);
    stop(&worker, &cancel);
    assert_eq!(hook3.stops.load(Ordering::Relaxed), 1);

    let started = Arc::new(AtomicBool::new(false));
    let startedByFactory = Arc::clone(&started);
    let panicFactory: HookFactoryFn = Arc::new(move || {
        startedByFactory.store(true, Ordering::Release);
        panic!("hook factory panic")
    });
    let (ctx, cancel) = Context::with_cancel();
    let worker = newHookWorkerWithRetry(
        ctx,
        "g1".to_string(),
        "h1".to_string(),
        Some(panicFactory),
        None,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );
    wait_until(Duration::from_secs(1), || started.load(Ordering::Acquire));
    std::thread::sleep(Duration::from_millis(1));
    let beforeCancel = Instant::now();
    stop(&worker, &cancel);
    assert!(beforeCancel.elapsed() < Duration::from_secs(1));
    store.Close();
}

#[test]
/// 单次请求处理 panic 后会话可继续。
fn TestHookWorkerLoopHandleRequestPanicRecover() {
    let hook = Arc::new(HookState::default());
    hook.pre(PreAction::Panic);
    hook.pre(PreAction::Return(Ok(PreSchedEventResult::default())));
    hook.sched(SchedAction::Panic);
    let (ctx, cancel) = Context::with_cancel();
    let worker = newHookWorkerWithRetry(
        ctx,
        "g1".to_string(),
        "h1".to_string(),
        Some(factory(Arc::clone(&hook))),
        None,
        Duration::from_millis(1),
        Duration::from_secs(60),
    );
    wait_until(Duration::from_secs(1), || {
        hook.starts.load(Ordering::Relaxed) == 1
    });

    let timer1 = TimerRecord {
        ID: "1".to_string(),
        Version: 1,
        EventStatus: SchedEventIdle.to_string(),
        ..TimerRecord::default()
    };
    let timer2 = TimerRecord {
        ID: timer1.ID.clone(),
        Version: 2,
        EventID: "event1".to_string(),
        EventStatus: SchedEventTrigger.to_string(),
        ..TimerRecord::default()
    };
    let (state, store) = scriptedStore(vec![Ok(())], vec![Ok(vec![timer2])]);

    let (req, rx) = request(timer1.clone(), "event1", store.clone());
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assertRetry(&response, &timer1.ID, "event1");
    assert_eq!(state.updateCalls.load(Ordering::Relaxed), 0);

    let (req, rx) = request(timer1.clone(), "event1", store);
    let response = sendWorkerRequestAndCheckResp(&worker, req, &rx);
    assertRetry(&response, &timer1.ID, "event1");
    assert_eq!(state.updateCalls.load(Ordering::Relaxed), 1);
    assert_eq!(state.listCalls.load(Ordering::Relaxed), 1);
    checkWorkerCounterValues(&worker, [2, 2, 0, 0, 1, 0]);

    stop(&worker, &cancel);
    assert_eq!(hook.starts.load(Ordering::Relaxed), 1);
    assert_eq!(hook.stops.load(Ordering::Relaxed), 1);
}

#[test]
/// Go 的 responseChan 在响应通道阻塞时仍会立即响应 context 取消。
fn TestWorkerBlockedResponseStopsOnCancel() {
    let store = NewMemoryTimerStore();
    let timer = TimerRecord {
        ID: "1".to_string(),
        EventStatus: SchedEventTrigger.to_string(),
        EventID: "event1".to_string(),
        ..TimerRecord::default()
    };
    let (response, _receiver) = crossbeam_channel::bounded(0);
    let request = TriggerEventRequest {
        eventID: timer.EventID.clone(),
        timer,
        store: store.clone(),
        resp: response,
    };
    let (ctx, cancel) = Context::with_cancel();
    let worker = newHookWorker(ctx, "g1".to_string(), "h1".to_string(), None, None);

    worker.ch.send(request).unwrap();
    wait_until(Duration::from_secs(1), || {
        worker.counters.triggerRequest.load(Ordering::Relaxed) == 1
    });
    std::thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    stop(&worker, &cancel);
    assert!(started.elapsed() < Duration::from_secs(1));
    store.Close();
}
