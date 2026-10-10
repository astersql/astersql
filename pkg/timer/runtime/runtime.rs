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

// Timer 分组运行时主循环。
//
// 维护定时器内存缓存，周期性全量/增量刷新存储，监听变更事件，
// 并将到期定时器投递给 Hook Worker 执行调度前后回调。

#![allow(non_snake_case, non_upper_case_globals)]

use crate::api::{
    And, CancelContext, Cond, Context, HookFactory, NewDefaultTimerClient, NewOptionalVal, Or,
    SchedEventIdle, TimerCond, TimerRecord, TimerStore, Timestamp, WatchTimerChan,
    WatchTimerEventCreate, WatchTimerEventDelete, WatchTimerEventUpdate, WatchTimerResponse,
};
use crate::cache::{
    NowFn, RuntimeProcStatus, TimersCache, newTimersCache, procIdle, procTriggering,
    procWaitTriggerClose, systemNow,
};
use crate::worker::{
    HookFactoryFn, HookWorker, TriggerEventRequest, newHookWorker, workerRespChanCap,
};
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub use crate::worker::TriggerEventResponse;

/// 全量刷新定时器缓存的周期。
pub const fullRefreshTimersInterval: Duration = Duration::from_secs(60);
/// 两次尝试触发之间的最大间隔。
pub const maxTriggerEventInterval: Duration = Duration::from_secs(60);
/// 两次尝试触发之间的最小间隔。
pub const minTriggerEventInterval: Duration = Duration::from_secs(1);
/// Watch 通道断开后重新订阅的等待间隔。
pub const reWatchInterval: Duration = Duration::from_secs(5);
/// 批量处理 Watch 响应的周期。
pub const batchProcessWatchRespInterval: Duration = Duration::from_secs(1);
/// Worker 通道满时推迟重试的间隔。
pub const retryBusyWorkerInterval: Duration = Duration::from_secs(5);
/// 检查等待关闭（WaitTriggerClose）定时器的周期。
pub const checkWaitCloseTimerInterval: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
/// 原子计数器，用于统计全量/增量刷新次数。
pub struct Counter(Arc<AtomicU64>);

impl Counter {
    /// 计数加一。
    pub fn Inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// 读取当前计数值。
    pub fn value(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// 分组运行时建造器：配置条件与 Hook 工厂后 `Build`。
pub struct TimerRuntimeBuilder {
    rt: TimerGroupRuntime,
}

/// 创建绑定分组 ID 与存储的运行时建造器。
pub fn NewTimerRuntimeBuilder(groupID: String, store: TimerStore) -> TimerRuntimeBuilder {
    let (workerRespSender, workerRespReceiver) = crossbeam_channel::bounded(workerRespChanCap);
    TimerRuntimeBuilder {
        rt: TimerGroupRuntime {
            inner: Arc::new(RuntimeInner {
                groupID,
                store,
                cond: RwLock::new(Arc::new(TimerCond::default())),
                factories: Mutex::new(HashMap::new()),
                cache: Mutex::new(newTimersCache()),
                workers: Mutex::new(HashMap::new()),
                workerRespSender,
                workerRespReceiver,
                nowFunc: RwLock::new(Arc::new(systemNow)),
                runState: Mutex::new(RunState::default()),
                fullRefreshTimerCounter: Counter::default(),
                partialRefreshTimerCounter: Counter::default(),
                retryLoopWait: Duration::from_secs(10),
                #[cfg(test)]
                recoveryEventSender: None,
            }),
        },
    }
}

impl TimerRuntimeBuilder {
    /// 设置列出/刷新定时器时使用的过滤条件。
    pub fn SetCond(self, cond: Arc<dyn Cond>) -> Self {
        *self
            .rt
            .inner
            .cond
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = cond;
        self
    }

    /// 注册指定 HookClass 的工厂，用于创建 Worker。
    pub fn RegisterHookFactory(self, hookClass: String, factory: HookFactory) -> Self {
        self.rt
            .inner
            .factories
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(hookClass, factory);
        self
    }

    /// 完成配置并得到可启动的分组运行时。
    pub fn Build(self) -> TimerGroupRuntime {
        self.rt
    }
}

#[derive(Clone)]
/// 一组定时器的运行时：缓存、Watch、触发与 Worker 响应处理。
pub struct TimerGroupRuntime {
    inner: Arc<RuntimeInner>,
}

/// 运行时共享状态：存储、条件、缓存、Worker 与循环控制。
struct RuntimeInner {
    groupID: String,
    store: TimerStore,
    cond: RwLock<Arc<dyn Cond>>,
    factories: Mutex<HashMap<String, HookFactory>>,
    cache: Mutex<TimersCache>,
    workers: Mutex<HashMap<String, HookWorker>>,
    workerRespSender: Sender<TriggerEventResponse>,
    workerRespReceiver: Receiver<TriggerEventResponse>,
    nowFunc: RwLock<NowFn>,
    runState: Mutex<RunState>,
    fullRefreshTimerCounter: Counter,
    partialRefreshTimerCounter: Counter,
    retryLoopWait: Duration,
    #[cfg(test)]
    recoveryEventSender: Option<Sender<RecoveryEvent>>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryEvent {
    PanicCaught(u64),
    RetryWaiting(u64),
    RetryDelayElapsed(u64),
    LoopResumed(u64),
}

#[derive(Default)]
/// 主循环线程的取消上下文与 JoinHandle。
struct RunState {
    ctx: Option<Context>,
    cancel: Option<CancelContext>,
    join: Option<JoinHandle<()>>,
}

impl TimerGroupRuntime {
    #[cfg(test)]
    pub(crate) fn setRetryLoopWait(&mut self, wait: Duration) {
        Arc::get_mut(&mut self.inner)
            .expect("retry wait must be configured before the runtime is shared")
            .retryLoopWait = wait;
    }

    #[cfg(test)]
    pub(crate) fn observeRecoveryEvents(&mut self) -> Receiver<RecoveryEvent> {
        let (sender, receiver) = crossbeam_channel::unbounded();
        Arc::get_mut(&mut self.inner)
            .expect("recovery events must be observed before the runtime is shared")
            .recoveryEventSender = Some(sender);
        receiver
    }

    /// 启动恢复循环线程；已运行则直接返回。
    pub fn Start(&self) {
        let mut state = self
            .inner
            .runState
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.ctx.is_some() {
            return;
        }
        let (ctx, cancel) = Context::with_cancel();
        state.ctx = Some(ctx.clone());
        state.cancel = Some(cancel);
        let runtime = self.clone();
        state.join = Some(thread::spawn(move || runtime.runRecoverLoop(ctx)));
    }

    /// 是否已启动（存在取消上下文）。
    pub fn Running(&self) -> bool {
        let state = self
            .inner
            .runState
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.ctx.is_some() && state.cancel.is_some()
    }

    /// 测试辅助：仅初始化上下文而不拉起循环线程。
    pub fn initCtx(&self) {
        let mut state = self
            .inner
            .runState
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.ctx.is_none() {
            let (ctx, cancel) = Context::with_cancel();
            state.ctx = Some(ctx);
            state.cancel = Some(cancel);
        }
    }

    /// 取消主循环并等待所有 Hook Worker 退出。
    pub fn Stop(&self) {
        let join = {
            let mut state = self
                .inner
                .runState
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(cancel) = state.cancel.take() {
                cancel.cancel();
            }
            state.join.take()
        };
        if let Some(join) = join {
            let _ = join.join();
        }
        let workers: Vec<HookWorker> = self
            .inner
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect();
        for worker in workers {
            worker.stopAndJoin();
        }
    }

    /// 捕获 panic 后按间隔重试的主循环外壳。
    fn runRecoverLoop(&self, ctx: Context) {
        let mut totalPanic = 0u64;
        while !ctx.is_cancelled() {
            if totalPanic > 0 {
                #[cfg(test)]
                self.notifyRecoveryEvent(RecoveryEvent::RetryWaiting(totalPanic));
                if !sleep(&ctx, self.inner.retryLoopWait) {
                    return;
                }
                #[cfg(test)]
                self.notifyRecoveryEvent(RecoveryEvent::RetryDelayElapsed(totalPanic));
            }
            let result = catch_unwind(AssertUnwindSafe(|| self.loopOnce(&ctx, totalPanic)));
            if result.is_ok() {
                return;
            }
            totalPanic += 1;
            #[cfg(test)]
            self.notifyRecoveryEvent(RecoveryEvent::PanicCaught(totalPanic));
        }
    }

    #[cfg(test)]
    fn notifyRecoveryEvent(&self, event: RecoveryEvent) {
        if let Some(sender) = &self.inner.recoveryEventSender {
            let _ = sender.send(event);
        }
    }

    /// 单次主循环：周期性全量刷新、关事件、批处理 Watch、尝试触发。
    fn loopOnce(&self, ctx: &Context, recoveredFromPanics: u64) {
        let mut watch = self.createWatchTimerChan(ctx);
        let mut batchResponses = Vec::with_capacity(1);
        let mut lastFullRefresh = Instant::now();
        let mut lastWaitClose = Instant::now();
        let mut lastBatch = Instant::now();
        let mut lastTryTriggerTime = systemNow() - chrono::Duration::seconds(60);
        let mut nextTry = Instant::now();
        let mut rewatchAt: Option<Instant> = None;

        // 启动时先全量灌入缓存，再进入事件驱动循环。
        self.fullRefreshTimersWithContext(ctx);
        #[cfg(test)]
        if recoveredFromPanics > 0 {
            self.notifyRecoveryEvent(RecoveryEvent::LoopResumed(recoveredFromPanics));
        }
        while !ctx.is_cancelled() {
            if lastFullRefresh.elapsed() >= fullRefreshTimersInterval {
                self.fullRefreshTimersWithContext(ctx);
                lastFullRefresh = Instant::now();
            }
            if lastWaitClose.elapsed() >= checkWaitCloseTimerInterval {
                self.tryCloseTriggeringTimersWithContext(ctx);
                lastWaitClose = Instant::now();
            }
            if lastBatch.elapsed() >= batchProcessWatchRespInterval {
                if self
                    .batchHandleWatchResponsesWithContext(ctx, std::mem::take(&mut batchResponses))
                {
                    nextTry = Instant::now();
                }
                lastBatch = Instant::now();
            }
            if Instant::now() >= nextTry {
                self.tryTriggerTimerEvents();
                lastTryTriggerTime = self.now();
                nextTry = Instant::now() + self.getNextTryTriggerDuration(lastTryTriggerTime);
            }
            if rewatchAt.is_some_and(|at| Instant::now() >= at) {
                watch = self.createWatchTimerChan(ctx);
                rewatchAt = None;
            }

            while let Ok(response) = self.inner.workerRespReceiver.try_recv() {
                self.handleWorkerResponse(response);
                nextTry = Instant::now() + self.getNextTryTriggerDuration(lastTryTriggerTime);
            }
            match watch.try_recv() {
                Ok(response) => batchResponses.push(response),
                Err(TryRecvError::Disconnected) => {
                    watch = crossbeam_channel::never();
                    rewatchAt = Some(Instant::now() + reWatchInterval);
                }
                Err(TryRecvError::Empty) => {}
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// 读取可注入的“当前时间”（测试可替换）。
    fn now(&self) -> Timestamp {
        self.inner
            .nowFunc
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())()
    }

    /// 按条件从存储全量列出并更新缓存。
    pub fn fullRefreshTimers(&self) {
        let ctx = self.context();
        self.fullRefreshTimersWithContext(&ctx);
    }

    /// 带上下文的全量刷新实现。
    fn fullRefreshTimersWithContext(&self, ctx: &Context) {
        self.inner.fullRefreshTimerCounter.Inc();
        let cond = self
            .inner
            .cond
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Ok(timers) = self.inner.store.List(ctx, Some(cond.as_ref())) else {
            return;
        };
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .fullUpdateTimers(timers);
    }

    /// 扫描到期定时器并投递到对应 Hook Worker。
    pub fn tryTriggerTimerEvents(&self) {
        let now = self.now();
        let mut readyTimers: Vec<(TimerRecord, Option<Timestamp>)> = Vec::new();
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            // 按 tryTriggerTime 顺序收集已到期、需触发的定时器。
            .iterTryTriggerTimers(|timer, tryTriggerTime, nextEventTime| {
                if tryTriggerTime > now {
                    return false;
                }
                if timer.EventStatus == SchedEventIdle
                    && (!timer.Enable
                        || nextEventTime.is_none()
                        || nextEventTime.is_some_and(|next| next > now))
                {
                    return true;
                }
                readyTimers.push((timer.clone(), nextEventTime));
                true
            });
        readyTimers.sort_by(|left, right| match (left.1, right.1) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(left), Some(right)) => left.cmp(&right),
        });

        let ctx = self.context();
        let mut retryTimerIDs = Vec::new();
        for (timer, _) in readyTimers {
            let Some(worker) = self.ensureWorker(&ctx, &timer.HookClass) else {
                continue;
            };
            let eventID = if timer.EventID.is_empty() {
                uuid::Uuid::new_v4().simple().to_string()
            } else {
                timer.EventID.clone()
            };
            let request = TriggerEventRequest {
                eventID: eventID.clone(),
                timer: timer.clone(),
                store: self.inner.store.clone(),
                resp: self.inner.workerRespSender.clone(),
            };
            match worker.ch.try_send(request) {
                Ok(()) => self
                    .inner
                    .cache
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .setTimerProcStatus(&timer.ID, procTriggering, eventID),
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                    retryTimerIDs.push(timer.ID)
                }
            }
        }
        if !retryTimerIDs.is_empty() {
            let retryAt = now
                + chrono::Duration::from_std(retryBusyWorkerInterval)
                    .expect("retry interval fits chrono duration");
            let mut cache = self
                .inner
                .cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for timerID in retryTimerIDs {
                cache.updateNextTryTriggerTime(&timerID, retryAt);
            }
        }
    }

    /// 按 HookClass 懒创建并缓存 Worker；无工厂则返回 None。
    fn ensureWorker(&self, ctx: &Context, hookClass: &str) -> Option<HookWorker> {
        if let Some(worker) = self
            .inner
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(hookClass)
            .cloned()
        {
            return Some(worker);
        }
        let factory = self
            .inner
            .factories
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(hookClass)
            .cloned()?;
        let store = self.inner.store.clone();
        let class = hookClass.to_string();
        let hookFn: HookFactoryFn = Arc::new(move || {
            factory(
                class.clone(),
                Box::new(NewDefaultTimerClient(store.clone())),
            )
        });
        let now = self
            .inner
            .nowFunc
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let worker = newHookWorker(
            ctx.clone(),
            self.inner.groupID.clone(),
            hookClass.to_string(),
            Some(hookFn),
            Some(now),
        );
        self.inner
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(hookClass.to_string(), worker.clone());
        Some(worker)
    }

    /// 根据最近触发时间与缓存中下一到期点计算下次扫描间隔。
    pub fn getNextTryTriggerDuration(&self, lastTryTriggerTime: Timestamp) -> Duration {
        let now = self.now();
        let sinceLastTrigger = (now - lastTryTriggerTime)
            .to_std()
            .unwrap_or(Duration::ZERO);
        let maxDuration = maxTriggerEventInterval.saturating_sub(sinceLastTrigger);
        if maxDuration.is_zero() {
            return Duration::ZERO;
        }
        let minDuration = minTriggerEventInterval.saturating_sub(sinceLastTrigger);
        let mut duration = maxDuration;
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iterTryTriggerTimers(|_, tryTriggerTime, _| {
                let interval = (tryTriggerTime - now).to_std().unwrap_or(Duration::ZERO);
                duration = duration.min(interval);
                false
            });
        duration.max(minDuration)
    }

    /// 根据 Worker 响应更新缓存状态、记录与重试时间。
    pub fn handleWorkerResponse(&self, response: TriggerEventResponse) {
        let mut cache = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !cache.hasTimer(&response.timerID) {
            return;
        }
        if let Some(timer) = response.newTimerRecord.Get() {
            match timer {
                Some(timer) => {
                    cache.updateTimer(timer);
                }
                None => {
                    cache.removeTimer(&response.timerID);
                }
            }
        }
        if !cache.hasTimer(&response.timerID) {
            return;
        }
        if response.success {
            cache.setTimerProcStatus(&response.timerID, procWaitTriggerClose, response.eventID);
        } else {
            cache.setTimerProcStatus(&response.timerID, procIdle, String::new());
            if let Some(retryAfter) = response.retryAfter.Get() {
                let retryAt = self.now()
                    + chrono::Duration::from_std(*retryAfter)
                        .expect("worker retry interval fits chrono duration");
                cache.updateNextTryTriggerTime(&response.timerID, retryAt);
            }
        }
    }

    /// 按 ID 集合增量刷新缓存；返回是否有变更。
    pub fn partialRefreshTimers(&self, timerIDs: &HashSet<String>) -> bool {
        let ctx = self.context();
        self.partialRefreshTimersWithContext(&ctx, timerIDs)
    }

    /// 带上下文的增量刷新：存储缺失的 ID 会从缓存删除。
    fn partialRefreshTimersWithContext(&self, ctx: &Context, timerIDs: &HashSet<String>) -> bool {
        if timerIDs.is_empty() {
            return false;
        }
        self.inner.partialRefreshTimerCounter.Inc();
        let cond = self.buildTimerIDsCond(timerIDs);
        let Ok(timers) = self.inner.store.List(ctx, Some(cond.as_ref())) else {
            return false;
        };
        let returned: HashSet<&str> = timers.iter().map(|timer| timer.ID.as_str()).collect();
        let mut cache = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for timerID in timerIDs {
            if !returned.contains(timerID.as_str()) {
                cache.removeTimer(timerID);
            }
        }
        cache.partialBatchUpdateTimers(timers)
    }

    /// 刷新处于 WaitTriggerClose 状态的定时器，推进收尾。
    pub fn tryCloseTriggeringTimers(&self) -> bool {
        let ctx = self.context();
        self.tryCloseTriggeringTimersWithContext(&ctx)
    }

    /// 带上下文的等待关闭定时器刷新。
    fn tryCloseTriggeringTimersWithContext(&self, ctx: &Context) -> bool {
        let ids = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .waitCloseTimerIDs()
            .clone();
        self.partialRefreshTimersWithContext(ctx, &ids)
    }

    /// 若存储支持 Watch 则订阅，否则返回永不触发的通道。
    pub fn createWatchTimerChan(&self, ctx: &Context) -> WatchTimerChan {
        if self.inner.store.WatchSupported() {
            self.inner.store.Watch(ctx)
        } else {
            crossbeam_channel::never()
        }
    }

    /// 批量处理 Watch 事件：创建/更新则增量刷新，删除则移出缓存。
    pub fn batchHandleWatchResponses(&self, responses: Vec<WatchTimerResponse>) -> bool {
        let ctx = self.context();
        self.batchHandleWatchResponsesWithContext(&ctx, responses)
    }

    /// 带上下文的 Watch 批处理实现。
    fn batchHandleWatchResponsesWithContext(
        &self,
        ctx: &Context,
        responses: Vec<WatchTimerResponse>,
    ) -> bool {
        if responses.is_empty() {
            return false;
        }
        let mut updateTimerIDs = HashSet::new();
        let mut delTimerIDs = HashSet::new();
        for response in responses {
            for event in response.Events {
                match event.Tp {
                    WatchTimerEventCreate | WatchTimerEventUpdate => {
                        updateTimerIDs.insert(event.TimerID);
                    }
                    WatchTimerEventDelete => {
                        delTimerIDs.insert(event.TimerID);
                    }
                    _ => {}
                }
            }
        }
        let mut changed = self.partialRefreshTimersWithContext(ctx, &updateTimerIDs);
        let mut cache = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for timerID in delTimerIDs {
            changed = cache.removeTimer(&timerID) || changed;
        }
        changed
    }

    /// 将 ID 集合与分组基础条件 AND 成查询条件。
    pub fn buildTimerIDsCond(&self, ids: &HashSet<String>) -> Arc<dyn Cond> {
        let children: Vec<Arc<dyn Cond>> = ids
            .iter()
            .map(|timerID| {
                Arc::new(TimerCond {
                    ID: NewOptionalVal(timerID.clone()),
                    ..TimerCond::default()
                }) as Arc<dyn Cond>
            })
            .collect();
        let base = self
            .inner
            .cond
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Arc::new(And(vec![base, Arc::new(Or(children))]))
    }

    /// 注入当前时间函数（同步到缓存）。
    pub fn setNowFunc(&self, nowFunc: NowFn) {
        *self
            .inner
            .nowFunc
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = nowFunc.clone();
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .setNowFunc(nowFunc);
    }

    /// 取得运行上下文；未启动时回退到 background。
    fn context(&self) -> Context {
        self.inner
            .runState
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ctx
            .clone()
            .unwrap_or_else(Context::background)
    }

    /// 测试辅助：直接更新缓存中的定时器。
    pub fn cacheUpdate(&self, timer: TimerRecord) -> bool {
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .updateTimer(&timer)
    }

    /// 测试辅助：设置定时器处理状态。
    pub fn cacheSetProcStatus(&self, timerID: &str, status: RuntimeProcStatus, eventID: &str) {
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .setTimerProcStatus(timerID, status, eventID.to_string());
    }

    /// 测试辅助：读取定时器处理状态。
    pub fn cacheProcStatus(&self, timerID: &str) -> Option<RuntimeProcStatus> {
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .item(timerID)
            .map(|item| item.procStatus)
    }

    /// 测试辅助：读取下次尝试触发时间。
    pub fn cacheNextTryTime(&self, timerID: &str) -> Option<Timestamp> {
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .item(timerID)
            .map(|item| item.nextTryTriggerTime)
    }

    /// 全量刷新次数。
    pub fn fullRefreshCount(&self) -> u64 {
        self.inner.fullRefreshTimerCounter.value()
    }

    /// 增量刷新次数。
    pub fn partialRefreshCount(&self) -> u64 {
        self.inner.partialRefreshTimerCounter.value()
    }
}

/// 可取消睡眠；未取消则返回 true。
fn sleep(ctx: &Context, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    while !ctx.is_cancelled() && Instant::now() < deadline {
        thread::sleep(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    !ctx.is_cancelled()
}
