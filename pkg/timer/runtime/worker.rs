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

// Hook Worker：异步执行定时器调度前后回调。
//
// 从通道接收触发请求，调用 Hook 的 `OnPreSchedEvent` / `OnSchedEvent`，
// 并通过存储更新事件状态，再把结果回传给分组运行时。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use crate::api::{
    Context, ErrTimerNotExist, ErrVersionNotMatch, EventExtra, Hook, NewOptionalVal, OptionalVal,
    PreSchedEventResult, SchedEventIdle, SchedEventTrigger, TimerRecord, TimerShedEvent,
    TimerStore, TimerUpdate, Timestamp,
};
use crate::cache::{NowFn, systemNow};
use crossbeam_channel::{Receiver, SendTimeoutError, Sender};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Worker 接收触发请求通道容量。
pub const workerRecvChanCap: usize = 128;
/// Worker 回传响应通道容量。
pub const workerRespChanCap: usize = 128;
/// 触发失败时的默认重试间隔。
pub const workerEventDefaultRetryInterval: Duration = Duration::from_secs(10);
/// 向响应通道发送时的最长阻塞时间。
pub const chanBlockInterval: Duration = Duration::from_secs(60);
/// Worker 会话 panic 后重启循环的等待间隔。
const hookWorkerRetryLoopInterval: Duration = Duration::from_secs(10);
/// 单次请求失败相关重试间隔（会话参数）。
const hookWorkerRetryRequestInterval: Duration = Duration::from_secs(5);

#[derive(Clone)]
/// 投递给 Hook Worker 的一次触发请求。
pub struct TriggerEventRequest {
    /// 本次调度事件 ID。
    pub eventID: String,
    /// 触发时的定时器快照。
    pub timer: TimerRecord,
    /// 用于读写定时器元数据的存储。
    pub store: TimerStore,
    /// 响应回传通道。
    pub resp: Sender<TriggerEventResponse>,
}

/// Go 风格别名，兼容机械翻译命名。
pub type triggerEventRequest = TriggerEventRequest;

impl TriggerEventRequest {
    /// 构造成功完成响应。
    pub fn DoneResponse(&self) -> TriggerEventResponse {
        TriggerEventResponse {
            timerID: self.timer.ID.clone(),
            eventID: self.eventID.clone(),
            success: true,
            ..TriggerEventResponse::default()
        }
    }

    /// 构造按默认间隔重试的失败响应。
    pub fn RetryDefaultResponse(&self) -> TriggerEventResponse {
        TriggerEventResponse {
            timerID: self.timer.ID.clone(),
            eventID: self.eventID.clone(),
            retryAfter: NewOptionalVal(workerEventDefaultRetryInterval),
            success: false,
            ..TriggerEventResponse::default()
        }
    }

    /// 元数据已变：立即重试，并附带最新记录（可为 None 表示已删除）。
    pub fn TimerMetaChangedResponse(&self, timer: Option<TimerRecord>) -> TriggerEventResponse {
        self.RetryDefaultResponse()
            .WithNewTimerRecord(timer)
            .WithRetryImmediately()
    }
}

#[derive(Clone, Debug, Default)]
/// Worker 处理触发请求后的响应。
pub struct TriggerEventResponse {
    /// 是否成功完成调度回调。
    pub success: bool,
    /// 对应定时器 ID。
    pub timerID: String,
    /// 对应事件 ID。
    pub eventID: String,
    /// 可选的最新定时器记录（None 表示已不存在）。
    pub newTimerRecord: OptionalVal<Option<TimerRecord>>,
    /// 失败时的重试延迟；未设置表示立即重试。
    pub retryAfter: OptionalVal<Duration>,
}

/// Go 风格别名，兼容机械翻译命名。
pub type triggerEventResponse = TriggerEventResponse;

impl TriggerEventResponse {
    /// 构造成功响应。
    pub fn done(timerID: &str, eventID: &str, timer: Option<TimerRecord>) -> Self {
        Self {
            success: true,
            timerID: timerID.to_string(),
            eventID: eventID.to_string(),
            newTimerRecord: NewOptionalVal(timer),
            retryAfter: OptionalVal::default(),
        }
    }

    /// 构造失败重试响应。
    pub fn retry(timerID: &str, eventID: &str, retryAfter: Option<Duration>) -> Self {
        Self {
            success: false,
            timerID: timerID.to_string(),
            eventID: eventID.to_string(),
            newTimerRecord: OptionalVal::default(),
            retryAfter: retryAfter.map_or_else(OptionalVal::default, NewOptionalVal),
        }
    }

    /// 清除重试延迟，表示立即重试。
    pub fn WithRetryImmediately(mut self) -> Self {
        self.retryAfter.Clear();
        self
    }

    /// 设置重试延迟。
    pub fn WithRetryAfter(mut self, duration: Duration) -> Self {
        self.retryAfter.Set(duration);
        self
    }

    /// 附带最新定时器记录。
    pub fn WithNewTimerRecord(mut self, timer: Option<TimerRecord>) -> Self {
        self.newTimerRecord.Set(timer);
        self
    }
}

/// 适配 `TimerShedEvent` 的本地事件视图。
struct TimerEvent {
    eventID: String,
    record: TimerRecord,
}

impl TimerShedEvent for TimerEvent {
    fn EventID(&self) -> String {
        self.eventID.clone()
    }

    fn Timer(&self) -> Option<TimerRecord> {
        Some(self.record.clone())
    }
}

#[derive(Default)]
/// Worker 内部计数器，便于测试与观测。
pub struct WorkerCounters {
    pub triggerRequest: AtomicU64,
    pub onPreSchedEvent: AtomicU64,
    pub onPreSchedEventErr: AtomicU64,
    pub onPreSchedEventDelay: AtomicU64,
    pub onSchedEvent: AtomicU64,
    pub onSchedEventErr: AtomicU64,
}

/// 创建 Hook 实例的工厂闭包类型。
pub type HookFactoryFn = Arc<dyn Fn() -> Box<dyn Hook> + Send + Sync + 'static>;

#[derive(Clone)]
/// 按 HookClass 运行的后台 Worker：接收触发请求并回调 Hook。
pub struct HookWorker {
    /// 向 Worker 投递请求的发送端。
    pub ch: Sender<TriggerEventRequest>,
    ctx: Context,
    join: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// 共享计数器。
    pub counters: Arc<WorkerCounters>,
}

/// Go 风格别名。
pub type hookWorker = HookWorker;

/// 使用默认重试间隔创建 Hook Worker。
pub fn newHookWorker(
    ctx: Context,
    groupID: String,
    hookClass: String,
    hookFn: Option<HookFactoryFn>,
    nowFunc: Option<NowFn>,
) -> HookWorker {
    newHookWorkerWithRetry(
        ctx,
        groupID,
        hookClass,
        hookFn,
        nowFunc,
        hookWorkerRetryLoopInterval,
        hookWorkerRetryRequestInterval,
    )
}

/// 创建可配置重试间隔的 Hook Worker 并启动循环线程。
pub fn newHookWorkerWithRetry(
    ctx: Context,
    _groupID: String,
    _hookClass: String,
    hookFn: Option<HookFactoryFn>,
    nowFunc: Option<NowFn>,
    retryLoopWait: Duration,
    retryRequestWait: Duration,
) -> HookWorker {
    let (sender, receiver) = crossbeam_channel::bounded(workerRecvChanCap);
    let worker = HookWorker {
        ch: sender,
        ctx: ctx.clone(),
        join: Arc::new(Mutex::new(None)),
        counters: Arc::new(WorkerCounters::default()),
    };
    let join = spawnWorkerLoop(
        ctx,
        receiver,
        hookFn,
        nowFunc.unwrap_or_else(|| Arc::new(systemNow)),
        Arc::clone(&worker.counters),
        retryLoopWait,
        retryRequestWait,
    );
    *worker
        .join
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(join);
    worker
}

/// 启动可从 panic 恢复的 Worker 循环线程。
fn spawnWorkerLoop(
    ctx: Context,
    receiver: Receiver<TriggerEventRequest>,
    hookFn: Option<HookFactoryFn>,
    nowFunc: NowFn,
    counters: Arc<WorkerCounters>,
    retryLoopWait: Duration,
    retryRequestWait: Duration,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !ctx.is_cancelled() {
            let result = catch_unwind(AssertUnwindSafe(|| {
                runWorkerSession(
                    &ctx,
                    &receiver,
                    hookFn.as_ref(),
                    nowFunc.as_ref(),
                    &counters,
                    retryRequestWait,
                )
            }));
            if result.is_ok() || ctx.is_cancelled() {
                break;
            }
            if !sleepWithContext(&ctx, retryLoopWait) {
                break;
            }
        }
    })
}

/// 可取消睡眠；完整睡完返回 true。
fn sleepWithContext(ctx: &Context, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    while !ctx.is_cancelled() {
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        thread::sleep((deadline - now).min(Duration::from_millis(10)));
    }
    false
}

/// 单次 Worker 会话：Start Hook、处理请求、最后 Stop。
fn runWorkerSession(
    ctx: &Context,
    receiver: &Receiver<TriggerEventRequest>,
    hookFn: Option<&HookFactoryFn>,
    nowFunc: &dyn Fn() -> Timestamp,
    counters: &WorkerCounters,
    _retryRequestWait: Duration,
) {
    let mut hook = hookFn.map(|factory| factory());
    let session = catch_unwind(AssertUnwindSafe(|| {
        if let Some(hook) = hook.as_mut() {
            hook.Start();
        }
        while !ctx.is_cancelled() {
            let request = match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(request) => request,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            };
            let response = catch_unwind(AssertUnwindSafe(|| match hook.as_mut() {
                Some(hook) => {
                    triggerEventWithCounters(ctx, Some(hook.as_mut()), &request, nowFunc, counters)
                }
                None => triggerEventWithCounters(ctx, None, &request, nowFunc, counters),
            }))
            .unwrap_or_else(|_| request.RetryDefaultResponse());
            responseChan(ctx, &request.resp, response);
        }
    }));
    let stopped = hook.as_mut().map(|hook| {
        catch_unwind(AssertUnwindSafe(|| {
            hook.Stop();
        }))
    });
    if let Err(panic) = session {
        std::panic::resume_unwind(panic);
    }
    if let Some(Err(panic)) = stopped {
        std::panic::resume_unwind(panic);
    }
}

impl HookWorker {
    /// 等待 Worker 线程结束（依赖上下文已取消）。
    pub fn stopAndJoin(&self) {
        if let Some(join) = self
            .join
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = join.join();
        }
    }

    /// 返回 Worker 绑定的取消上下文。
    pub fn context(&self) -> &Context {
        &self.ctx
    }
}

/// 执行一次触发流程（使用临时计数器）。
pub fn triggerEvent(
    ctx: &Context,
    hook: Option<&mut dyn Hook>,
    req: &TriggerEventRequest,
    nowFunc: &dyn Fn() -> Timestamp,
) -> TriggerEventResponse {
    triggerEventWithCounters(ctx, hook, req, nowFunc, &WorkerCounters::default())
}

/// 触发核心：处理手动请求超时、PreSched、落库为 Trigger、OnSched。
fn triggerEventWithCounters(
    ctx: &Context,
    mut hook: Option<&mut dyn Hook>,
    req: &TriggerEventRequest,
    nowFunc: &dyn Fn() -> Timestamp,
    counters: &WorkerCounters,
) -> TriggerEventResponse {
    counters.triggerRequest.fetch_add(1, Ordering::Relaxed);
    let mut timer = req.timer.clone();

    // 手动触发超时则标记已处理并返回元数据变更响应。
    if timer.ManualRequest.IsManualRequesting() {
        let timedOut = timer
            .ManualRequest
            .ManualRequestTime
            .and_then(|start| {
                chrono::Duration::from_std(timer.ManualRequest.ManualTimeout)
                    .ok()
                    .and_then(|timeout| start.checked_add_signed(timeout))
            })
            .is_some_and(|timeout| nowFunc() > timeout);
        if timedOut {
            let mut update = TimerUpdate::default();
            update
                .ManualRequest
                .Set(timer.ManualRequest.SetProcessed(String::new()));
            update.CheckVersion.Set(timer.Version);
            match req.store.Update(ctx, &timer.ID, Some(update)) {
                Ok(()) => match req.store.GetByID(ctx, &timer.ID) {
                    Ok(updated) => return req.TimerMetaChangedResponse(Some(updated)),
                    Err(error) if error == ErrTimerNotExist => {
                        return req.TimerMetaChangedResponse(None);
                    }
                    Err(_) => return req.RetryDefaultResponse(),
                },
                Err(error) if error == ErrTimerNotExist => {
                    return req.TimerMetaChangedResponse(Some(timer));
                }
                Err(_) => return req.RetryDefaultResponse(),
            }
        }
    }

    // Idle 状态先走 OnPreSchedEvent，再把事件状态写为 Trigger。
    if timer.EventStatus == SchedEventIdle {
        let mut preResult = PreSchedEventResult::default();
        if let Some(hook) = hook.as_deref_mut() {
            counters.onPreSchedEvent.fetch_add(1, Ordering::Relaxed);
            let event = TimerEvent {
                eventID: req.eventID.clone(),
                record: timer.clone(),
            };
            match hook.OnPreSchedEvent(ctx, &event) {
                Ok(result) if result.Delay > Duration::ZERO => {
                    counters
                        .onPreSchedEventDelay
                        .fetch_add(1, Ordering::Relaxed);
                    return req.RetryDefaultResponse().WithRetryAfter(result.Delay);
                }
                Ok(result) => preResult = result,
                Err(_) => {
                    counters.onPreSchedEventErr.fetch_add(1, Ordering::Relaxed);
                    return req.RetryDefaultResponse();
                }
            }
        }

        let update = buildEventUpdate(req, preResult, nowFunc);
        if let Err(error) = req.store.Update(ctx, &timer.ID, Some(update)) {
            if error == ErrVersionNotMatch {
                match req.store.GetByID(ctx, &timer.ID) {
                    Ok(updated) => return req.TimerMetaChangedResponse(Some(updated)),
                    Err(getError) if getError == ErrTimerNotExist => {
                        return req.TimerMetaChangedResponse(None);
                    }
                    Err(_) => {}
                }
            }
            if error == ErrTimerNotExist {
                return req.TimerMetaChangedResponse(None);
            }
            return req.RetryDefaultResponse();
        }
    }

    timer = match req.store.GetByID(ctx, &timer.ID) {
        Ok(timer) => timer,
        Err(error) if error == ErrTimerNotExist => return req.TimerMetaChangedResponse(None),
        Err(_) => return req.RetryDefaultResponse(),
    };
    if timer.EventID != req.eventID {
        return req.TimerMetaChangedResponse(Some(timer));
    }

    // EventID 匹配后执行 OnSchedEvent；失败则带最新记录重试。
    if let Some(hook) = hook.as_deref_mut() {
        counters.onSchedEvent.fetch_add(1, Ordering::Relaxed);
        let event = TimerEvent {
            eventID: req.eventID.clone(),
            record: timer.clone(),
        };
        if hook.OnSchedEvent(ctx, &event).is_err() {
            counters.onSchedEventErr.fetch_add(1, Ordering::Relaxed);
            return req.RetryDefaultResponse().WithNewTimerRecord(Some(timer));
        }
    }

    req.DoneResponse().WithNewTimerRecord(Some(timer))
}

/// 在取消或断开前阻塞发送响应。
fn responseChan(
    ctx: &Context,
    channel: &Sender<TriggerEventResponse>,
    mut response: TriggerEventResponse,
) -> bool {
    // Go selects the response send and ctx.Done concurrently. Context in this
    // port exposes cancellation as a flag, so use a short bounded send to keep
    // cancellation responsive while preserving the long blocked-send window.
    let cancellationPollInterval = chanBlockInterval.min(Duration::from_millis(10));
    loop {
        if ctx.is_cancelled() {
            return false;
        }
        match channel.send_timeout(response, cancellationPollInterval) {
            Ok(()) => return true,
            Err(SendTimeoutError::Timeout(returned)) => response = returned,
            Err(SendTimeoutError::Disconnected(_)) => return false,
        }
    }
}

/// 根据 PreSched 结果构造将定时器置为 Trigger 的更新。
pub fn buildEventUpdate(
    req: &TriggerEventRequest,
    result: PreSchedEventResult,
    nowFunc: &dyn Fn() -> Timestamp,
) -> TimerUpdate {
    let mut update = TimerUpdate::default();
    update.EventStatus.Set(SchedEventTrigger.to_string());
    update.EventID.Set(req.eventID.clone());
    update.EventStart.Set(Some(nowFunc()));
    update.EventData.Set(result.EventData);
    update.CheckVersion.Set(req.timer.Version);

    let mut eventExtra = EventExtra {
        EventWatermark: req.timer.Watermark,
        ..EventExtra::default()
    };
    if req.timer.ManualRequest.IsManualRequesting() {
        eventExtra.EventManualRequestID = req.timer.ManualRequest.ManualRequestID.clone();
        update
            .ManualRequest
            .Set(req.timer.ManualRequest.SetProcessed(req.eventID.clone()));
    }
    update.EventExtra.Set(eventExtra);
    update
}
