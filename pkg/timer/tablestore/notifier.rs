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

// 基于 etcd 的定时器变更通知。
//
// 将 create/update/delete 事件批量写入带租约的键，供集群内其他节点前缀监听，
// 实现表存储之上的跨节点定时器事件广播。

use astersql_timer_api as api;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 单次向 etcd 写入通知事件的超时时间。
const notifyTimeout: Duration = Duration::from_secs(20);
/// 连续两次通知之间的最小间隔，避免写放大。
const minNotifyInterval: Duration = Duration::from_secs(1);
/// etcd lease TTL（秒）：通知键过期后自动清理。
const etcdNotifyKeyTTLSeconds: i64 = 60;
/// 序列化到 etcd 的创建事件类型名。
const watchTimerEventCreate: &str = "create";
/// 序列化到 etcd 的更新事件类型名。
const watchTimerEventUpdate: &str = "update";
/// 序列化到 etcd 的删除事件类型名。
const watchTimerEventDelete: &str = "delete";

#[derive(Clone, Debug, Eq, PartialEq)]
/// 写入 etcd 的通知载荷：事件类型、定时器 ID 与时间戳。
pub struct EtcdNotifyEvent {
    /// 事件类型字符串（create/update/delete）。
    pub tp: String,
    /// 关联的定时器 ID。
    pub timer_id: String,
    /// 事件产生时的 Unix 秒时间戳。
    pub timestamp: i64,
}

impl EtcdNotifyEvent {
    /// 将 etcd 通知载荷转换为运行时使用的 `WatchTimerEvent`。
    pub fn toWatchEvent(&self) -> api::TimerResult<api::WatchTimerEvent> {
        if self.timer_id.is_empty() {
            return Err(api::TimerError::message("timerID is empty"));
        }
        let tp = match self.tp.as_str() {
            watchTimerEventCreate => api::WatchTimerEventCreate,
            watchTimerEventUpdate => api::WatchTimerEventUpdate,
            watchTimerEventDelete => api::WatchTimerEventDelete,
            other => {
                return Err(api::TimerError::message(format!(
                    "invalid WatchTimerEventType: {other}"
                )));
            }
        };
        Ok(api::WatchTimerEvent {
            Tp: tp,
            TimerID: self.timer_id.clone(),
        })
    }
}

/// 由 API 事件类型构造带时间戳的 etcd 通知事件。
fn newNotifyEvent(
    tp: api::WatchTimerEventType,
    timer_id: &str,
) -> api::TimerResult<EtcdNotifyEvent> {
    let name = match tp {
        api::WatchTimerEventCreate => watchTimerEventCreate,
        api::WatchTimerEventUpdate => watchTimerEventUpdate,
        api::WatchTimerEventDelete => watchTimerEventDelete,
        other => {
            return Err(api::TimerError::message(format!(
                "invalid WatchTimerEventType: {other}, timer: {timer_id}"
            )));
        }
    };
    Ok(EtcdNotifyEvent {
        tp: name.to_string(),
        timer_id: timer_id.to_string(),
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    })
}

/// Canonical etcd adapter boundary. The adapter owns wire JSON decoding for
/// prefix watches and exposes the canonical timer channel to callers.
/// etcd 适配边界：租约、前缀监听与批量写入通知。
///
/// 适配器负责前缀 watch 的 JSON 解码，并向调用方暴露规范的定时器通道。
pub trait EtcdClient: Send + Sync {
    /// 申请租约，返回 lease ID。
    fn grant(&self, ttl_seconds: i64) -> Result<i64, String>;
    /// 保持租约存活；通道断开表示 keep-alive 失败。
    fn keep_alive(&self, lease_id: i64) -> Result<mpsc::Receiver<()>, String>;
    /// 监听指定前缀上的通知键变更。
    fn watch_prefix(&self, prefix: &str, ctx: &api::Context) -> api::WatchTimerChan;
    /// 将一批通知事件写入指定键（绑定 lease）。
    fn put_events(
        &self,
        key: &str,
        events: &[EtcdNotifyEvent],
        lease_id: i64,
        timeout: Duration,
    ) -> Result<(), String>;
}

/// 通知器可变状态：关闭标记、待发送事件与后台线程句柄。
struct State {
    closed: bool,
    events: Vec<EtcdNotifyEvent>,
    notify_worker: Option<JoinHandle<()>>,
    watch_workers: Vec<JoinHandle<()>>,
}

/// 通知器共享内部：客户端、键前缀与唤醒通道。
struct NotifierInner {
    client: Arc<dyn EtcdClient>,
    key_prefix: String,
    key: String,
    state: Mutex<State>,
    wake_tx: mpsc::SyncSender<()>,
    wake_rx: Mutex<mpsc::Receiver<()>>,
    closed: AtomicBool,
}

/// 基于 etcd 的定时器变更通知器实现。
pub struct EtcdNotifier {
    inner: Arc<NotifierInner>,
}

/// 为本进程通知键生成唯一后缀。
static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

/// 创建 etcd 通知器并启动后台 `notifyLoop`。
pub fn NewEtcdNotifier(
    cluster_id: u64,
    client: Arc<dyn EtcdClient>,
) -> Arc<dyn api::TimerWatchEventNotifier> {
    let key_prefix = format!("/tidb/timer/cluster/{cluster_id}/notify/");
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let key = format!(
        "{}{:08x}-{:032x}-{:016x}",
        key_prefix,
        std::process::id(),
        created_at,
        NEXT_KEY.fetch_add(1, Ordering::Relaxed)
    );
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    let notifier = Arc::new(EtcdNotifier {
        inner: Arc::new(NotifierInner {
            client,
            key_prefix,
            key,
            state: Mutex::new(State {
                closed: false,
                events: Vec::with_capacity(8),
                notify_worker: None,
                watch_workers: Vec::new(),
            }),
            wake_tx,
            wake_rx: Mutex::new(wake_rx),
            closed: AtomicBool::new(false),
        }),
    });
    let inner = Arc::clone(&notifier.inner);
    let worker = std::thread::spawn(move || notifyLoop(inner));
    notifier
        .inner
        .state
        .lock()
        .expect("notifier lock poisoned")
        .notify_worker = Some(worker);
    notifier
}

/// 后台循环：维持租约、节流后批量把事件写入 etcd。
fn notifyLoop(inner: Arc<NotifierInner>) {
    let mut lease_id = 0;
    let mut keep_alive: Option<mpsc::Receiver<()>> = None;
    let mut last_notify = Instant::now() - minNotifyInterval;
    // 检查 keep-alive 是否断开，必要时重建租约。
    while !inner.closed.load(Ordering::Acquire) {
        if let Some(receiver) = keep_alive.as_ref() {
            match receiver.try_recv() {
                Ok(()) | Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    lease_id = 0;
                    keep_alive = None;
                }
            }
        }
        let wake = inner
            .wake_rx
            .lock()
            .expect("notifier wake lock poisoned")
            .recv_timeout(Duration::from_millis(20));
        match wake {
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        // 节流：距上次通知不足最小间隔则等待。
        let elapsed = last_notify.elapsed();
        if elapsed < minNotifyInterval {
            let deadline = Instant::now() + (minNotifyInterval - elapsed);
            while Instant::now() < deadline {
                if inner.closed.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        last_notify = Instant::now();
        // 尚无有效租约时先 grant + keep_alive。
        if lease_id == 0 {
            let Ok(new_lease) = inner.client.grant(etcdNotifyKeyTTLSeconds) else {
                continue;
            };
            let Ok(new_keep_alive) = inner.client.keep_alive(new_lease) else {
                continue;
            };
            lease_id = new_lease;
            keep_alive = Some(new_keep_alive);
        }
        sendEvents(&inner, lease_id);
    }
}

/// 取出并清空当前积压的待发送事件。
fn takeEvents(inner: &NotifierInner) -> Vec<EtcdNotifyEvent> {
    let mut state = inner.state.lock().expect("notifier lock poisoned");
    std::mem::take(&mut state.events)
}

/// 将积压事件通过当前 lease 写入 etcd。
fn sendEvents(inner: &NotifierInner, lease_id: i64) {
    let events = takeEvents(inner);
    if !events.is_empty() {
        let _ = inner
            .client
            .put_events(&inner.key, &events, lease_id, notifyTimeout);
    }
}

/// 实现 `TimerWatchEventNotifier`：监听、入队通知与关闭。
impl api::TimerWatchEventNotifier for EtcdNotifier {
    /// 监听本集群通知前缀。
    fn Watch(&self, ctx: &api::Context) -> api::WatchTimerChan {
        let mut state = self.inner.state.lock().expect("notifier lock poisoned");
        if state.closed {
            let notifier = api::NewMemTimerWatchEventNotifier();
            notifier.Close();
            return notifier.Watch(ctx);
        }

        let (watch_ctx, cancel) = api::Context::with_cancel();
        let receiver = self
            .inner
            .client
            .watch_prefix(&self.inner.key_prefix, &watch_ctx);
        let inner = Arc::clone(&self.inner);
        let caller_ctx = ctx.clone();
        state.watch_workers.push(std::thread::spawn(move || {
            while !inner.closed.load(Ordering::Acquire) && !caller_ctx.is_cancelled() {
                std::thread::sleep(Duration::from_millis(10));
            }
            cancel.cancel();
        }));
        receiver
    }

    /// 将事件入队并唤醒后台发送循环。
    fn Notify(&self, tp: api::WatchTimerEventType, timer_id: &str) {
        let Ok(event) = newNotifyEvent(tp, timer_id) else {
            return;
        };
        let mut state = self.inner.state.lock().expect("notifier lock poisoned");
        if state.closed {
            return;
        }
        state.events.push(event);
        drop(state);
        let _ = self.inner.wake_tx.try_send(());
    }

    /// 标记关闭并等待后台线程退出。
    fn Close(&self) {
        let (notify_worker, watch_workers) = {
            let mut state = self.inner.state.lock().expect("notifier lock poisoned");
            if state.closed {
                return;
            }
            state.closed = true;
            self.inner.closed.store(true, Ordering::Release);
            let _ = self.inner.wake_tx.try_send(());
            (
                state.notify_worker.take(),
                std::mem::take(&mut state.watch_workers),
            )
        };
        if let Some(worker) = notify_worker {
            let _ = worker.join();
        }
        for worker in watch_workers {
            let _ = worker.join();
        }
    }
}
