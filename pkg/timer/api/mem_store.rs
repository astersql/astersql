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

// 内存版定时器存储实现。
//
// 以 HashMap 保存命名空间与 ID 索引，并通过 `MemTimerWatchEventNotifier`
// 向订阅者推送创建/更新/删除事件；主要用于测试与本地原型。

use crate::error::{ErrTimerExists, ErrTimerNotExist, TimerError, TimerResult};
use crate::store::{
    Cond, Context, TimerStore, TimerStoreCore, TimerUpdate, TimerWatchEventNotifier,
    WatchTimerChan, WatchTimerEvent, WatchTimerEventCreate, WatchTimerEventDelete,
    WatchTimerEventType, WatchTimerEventUpdate, WatchTimerResponse,
};
use crate::timer::{
    SchedEventIdle, TimerLocation, TimerRecord, in_location, now_timestamp, parse_location,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// 内存存储内部数据：按命名空间+Key 与按 ID 双索引。
struct MemoryStoreData {
    namespaces: HashMap<String, HashMap<String, TimerRecord>>,
    id2Timers: HashMap<String, TimerRecord>,
}

/// 实现 `TimerStoreCore` 的内存存储核心。
pub struct MemoryStoreCore {
    data: Mutex<MemoryStoreData>,
    notifier: Arc<MemTimerWatchEventNotifier>,
}

/// 构造空的内存 TimerStore。
pub fn NewMemoryTimerStore() -> TimerStore {
    TimerStore::from_core(MemoryStoreCore {
        data: Mutex::new(MemoryStoreData {
            namespaces: HashMap::new(),
            id2Timers: HashMap::new(),
        }),
        notifier: Arc::new(MemTimerWatchEventNotifier::new()),
    })
}

// Create 分配 ID/版本；Update 应用补丁并递增版本；Delete 同步清理双索引。
impl TimerStoreCore for MemoryStoreCore {
    // 拒绝调用方预填 ID/Version/CreateTime；校验后写入并通知 Create。
    fn Create(&self, _ctx: &Context, record: Option<TimerRecord>) -> TimerResult<String> {
        let mut record = record.ok_or_else(|| TimerError::message("timer should not be nil"))?;
        if !record.ID.is_empty() {
            return Err(TimerError::message(
                "ID should not be specified when create record",
            ));
        }
        if record.Version != 0 {
            return Err(TimerError::message(
                "Version should not be specified when create record",
            ));
        }
        if record.CreateTime.is_some() {
            return Err(TimerError::message(
                "CreateTime should not be specified when create record",
            ));
        }
        record.Validate()?;

        record.ID = uuid::Uuid::new_v4().simple().to_string();
        record.Location = Some(getMemStoreTimeZoneLoc(&record.TimeZone));
        record.Version = 1;
        record.CreateTime = Some(now_timestamp());
        if record.EventStatus.is_empty() {
            record.EventStatus = SchedEventIdle.to_string();
        }
        normalizeTimeFields(&mut record);

        let timer_id = record.ID.clone();
        {
            let mut data = self.data.lock();
            // ID 或 (Namespace, Key) 任一冲突则视为已存在。
            if data.id2Timers.contains_key(&timer_id)
                || data
                    .namespaces
                    .get(&record.Namespace)
                    .is_some_and(|namespace| namespace.contains_key(&record.Key))
            {
                return Err(ErrTimerExists);
            }
            data.id2Timers.insert(timer_id.clone(), record.clone());
            data.namespaces
                .entry(record.Namespace.clone())
                .or_default()
                .insert(record.Key.clone(), record);
        }
        self.notifier.Notify(WatchTimerEventCreate, &timer_id);
        Ok(timer_id)
    }

    // 遍历所有命名空间，按 Cond 过滤（无 Cond 则全量）。
    fn List(&self, _ctx: &Context, cond: Option<&dyn Cond>) -> TimerResult<Vec<TimerRecord>> {
        let data = self.data.lock();
        Ok(data
            .namespaces
            .values()
            .flat_map(HashMap::values)
            .filter(|timer| cond.is_none_or(|condition| condition.Match(timer)))
            .cloned()
            .collect())
    }

    // 加锁取出记录、apply 补丁、规范化时间字段后写回并 Notify。
    fn Update(
        &self,
        _ctx: &Context,
        timerID: &str,
        update: Option<TimerUpdate>,
    ) -> TimerResult<()> {
        let update = update.ok_or_else(|| TimerError::message("update should not be nil"))?;
        {
            let mut data = self.data.lock();
            let record = data
                .id2Timers
                .get(timerID)
                .cloned()
                .ok_or(ErrTimerNotExist)?;
            let mut updated = update.apply(&record)?;
            normalizeTimeFields(&mut updated);
            updated.Validate()?;
            updated.Version += 1;
            data.id2Timers.insert(timerID.to_string(), updated.clone());
            if let Some(namespace) = data.namespaces.get_mut(&record.Namespace) {
                namespace.insert(record.Key.clone(), updated);
            }
        }
        self.notifier.Notify(WatchTimerEventUpdate, timerID);
        Ok(())
    }

    // 同时从 id 索引与命名空间索引移除；空命名空间一并删除。
    fn Delete(&self, _ctx: &Context, timerID: &str) -> TimerResult<bool> {
        {
            let mut data = self.data.lock();
            let Some(record) = data.id2Timers.remove(timerID) else {
                return Ok(false);
            };
            let remove_namespace =
                if let Some(namespace) = data.namespaces.get_mut(&record.Namespace) {
                    namespace.remove(&record.Key);
                    namespace.is_empty()
                } else {
                    false
                };
            if remove_namespace {
                data.namespaces.remove(&record.Namespace);
            }
        }
        self.notifier.Notify(WatchTimerEventDelete, timerID);
        Ok(true)
    }

    fn WatchSupported(&self) -> bool {
        true
    }

    // 外部有界通道 + 内部缓冲；worker 在取消/shutdown 时退出并注销。
    fn Watch(&self, ctx: &Context) -> WatchTimerChan {
        self.notifier.Watch(ctx)
    }

    // 标记关闭、清空订阅、释放 shutdown 发送端并等待 workers。
    fn Close(&self) {
        self.notifier.Close();
    }
}

/// 单个 Watch 订阅者：持有上下文与发送端。
struct Watcher {
    ctx: Context,
    sender: crossbeam_channel::Sender<WatchTimerResponse>,
}

/// Notifier 可变状态：关闭标志、ID 分配与订阅表。
struct NotifierState {
    closed: bool,
    next_id: u64,
    watchers: HashMap<u64, Watcher>,
}

/// Notifier 共享内部：状态、shutdown 发送端与 worker 句柄。
struct NotifierInner {
    state: Mutex<NotifierState>,
    shutdown: Mutex<Option<crossbeam_channel::Sender<()>>>,
    workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

/// 内存 Watch 通知器：同步 try_send，通道满时异步投递。
pub struct MemTimerWatchEventNotifier {
    inner: Arc<NotifierInner>,
    shutdown_rx: crossbeam_channel::Receiver<()>,
}

impl MemTimerWatchEventNotifier {
    /// 创建未关闭的通知器与 shutdown 通道。
    fn new() -> Self {
        let (shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(0);
        Self {
            inner: Arc::new(NotifierInner {
                state: Mutex::new(NotifierState {
                    closed: false,
                    next_id: 0,
                    watchers: HashMap::new(),
                }),
                shutdown: Mutex::new(Some(shutdown_tx)),
                workers: Mutex::new(Vec::new()),
            }),
            shutdown_rx,
        }
    }
}

/// 构造可共享的内存 Watch 通知器。
pub fn NewMemTimerWatchEventNotifier() -> Arc<dyn TimerWatchEventNotifier> {
    Arc::new(MemTimerWatchEventNotifier::new())
}

// Watch 注册订阅并起转发线程；Notify 广播；Close 清理并 join workers。
impl TimerWatchEventNotifier for MemTimerWatchEventNotifier {
    fn Watch(&self, ctx: &Context) -> WatchTimerChan {
        let (external_sender, receiver) = crossbeam_channel::bounded(0);
        let (sender, internal_receiver) = crossbeam_channel::bounded(8);
        let watcher_id = {
            let mut state = self.inner.state.lock();
            if state.closed {
                return receiver;
            }
            let watcher_id = state.next_id;
            state.next_id += 1;
            state.watchers.insert(
                watcher_id,
                Watcher {
                    ctx: ctx.clone(),
                    sender,
                },
            );
            watcher_id
        };

        let inner = Arc::clone(&self.inner);
        let shutdown = self.shutdown_rx.clone();
        let watcher_ctx = ctx.clone();
        let worker = std::thread::spawn(move || {
            'watch: while !watcher_ctx.is_cancelled() {
                let response = crossbeam_channel::select! {
                    recv(shutdown) -> _ => break,
                    recv(internal_receiver) -> response => match response {
                        Ok(response) => response,
                        Err(_) => break,
                    },
                    default(Duration::from_millis(10)) => continue,
                };
                loop {
                    if watcher_ctx.is_cancelled() {
                        break 'watch;
                    }
                    crossbeam_channel::select! {
                        recv(shutdown) -> _ => break 'watch,
                        send(external_sender, response.clone()) -> sent => {
                            if sent.is_err() {
                                break 'watch;
                            }
                            break;
                        },
                        default(Duration::from_millis(10)) => continue,
                    }
                }
            }
            inner.state.lock().watchers.remove(&watcher_id);
        });
        self.inner.workers.lock().push(worker);
        receiver
    }

    // try_send 成功则保留订阅；Full 时开线程阻塞发送；Disconnected 移除。
    fn Notify(&self, tp: WatchTimerEventType, timerID: &str) {
        let response = WatchTimerResponse {
            Events: vec![WatchTimerEvent {
                Tp: tp,
                TimerID: timerID.to_string(),
            }],
        };
        let mut state = self.inner.state.lock();
        if state.closed {
            return;
        }
        let mut async_sends = Vec::new();
        state.watchers.retain(|_, watcher| {
            if watcher.ctx.is_cancelled() {
                return false;
            }
            match watcher.sender.try_send(response.clone()) {
                Ok(()) => true,
                Err(crossbeam_channel::TrySendError::Disconnected(_)) => false,
                Err(crossbeam_channel::TrySendError::Full(response)) => {
                    async_sends.push((watcher.ctx.clone(), watcher.sender.clone(), response));
                    true
                }
            }
        });
        drop(state);
        for (ctx, sender, response) in async_sends {
            let shutdown = self.shutdown_rx.clone();
            let worker = std::thread::spawn(move || {
                loop {
                    if ctx.is_cancelled() {
                        return;
                    }
                    crossbeam_channel::select! {
                        recv(shutdown) -> _ => return,
                        send(sender, response.clone()) -> _ => return,
                        default(Duration::from_millis(10)) => continue,
                    }
                }
            });
            self.inner.workers.lock().push(worker);
        }
    }

    fn Close(&self) {
        {
            let mut state = self.inner.state.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            state.watchers.clear();
        }
        self.inner.shutdown.lock().take();
        for worker in self.inner.workers.lock().drain(..) {
            let _ = worker.join();
        }
    }
}

/// 解析时区字符串，失败则回退系统时区。
pub fn getMemStoreTimeZoneLoc(tz: &str) -> TimerLocation {
    parse_location(tz).unwrap_or_else(|_| parse_location("").expect("system timezone is valid"))
}

/// 将 Watermark/EventStart/CreateTime 转换到记录 Location。
pub fn normalizeTimeFields(record: &mut TimerRecord) {
    let Some(location) = record.Location.clone() else {
        return;
    };
    record.Watermark = record
        .Watermark
        .as_ref()
        .map(|value| in_location(value, Some(&location)));
    record.EventStart = record
        .EventStart
        .as_ref()
        .map(|value| in_location(value, Some(&location)));
    record.CreateTime = record
        .CreateTime
        .as_ref()
        .map(|value| in_location(value, Some(&location)));
}
