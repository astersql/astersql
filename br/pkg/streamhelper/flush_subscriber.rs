// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Flush 订阅：按 Store 拓扑维护 log-backup flush 事件通道。
//!
//! 对应 Go `flush_subscriber.go`。gRPC 流仍由环境边界承载；本模块完整维护
//! Store 拓扑、连接错误、重试和事件隧道生命周期。

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use astersql_br_pkg_streamhelper_spans::{Span, Valued};

use crate::advancer_env::Env;
use crate::regioniter::Store;

/// 清理订阅时的超时上限（与 Go `clearSubscriberTimeOut` 一致）。
pub const clearSubscriberTimeOut: Duration = Duration::from_secs(60);
/// 单路 flush 订阅允许空闲的最长时间（默认 10 分钟）。
pub const subscriptionIdleTimeout: Duration = Duration::from_secs(10 * 60);

/// 构造期选项：闭包在 `NewSubscriber` 时依次应用到实例。
pub type SubscriberConfig = Box<dyn FnMut(&mut FlushSubscriber) + Send>;

/// Go 侧注入 master context；精简版为空操作占位，保持 API 形状。
pub fn WithMasterContext(_ctx_token: ()) -> SubscriberConfig {
    Box::new(|_fs: &mut FlushSubscriber| {})
}

/// 覆盖默认空闲超时，供测试缩短等待。
pub fn WithSubscriptionIdleTimeout(timeout: Duration) -> SubscriberConfig {
    Box::new(move |fs: &mut FlushSubscriber| {
        fs.subscriptionIdleTimeout = timeout;
    })
}

/// 单个 Store 上的订阅元数据；BootAt 变化视为节点重启需重建。
struct Subscription {
    storeBootAt: u64,
    pendingError: Arc<Mutex<Option<String>>>,
    stop: Option<Sender<()>>,
    background: Option<JoinHandle<()>>,
}

/// 维护 store→订阅 映射，并把 flush 事件送入单消费者通道。
pub struct FlushSubscriber {
    env: Arc<dyn Env>,
    subscriptions: HashMap<u64, Subscription>,
    /// 生产者端；`PushEvent` 写入。
    eventsTunnel: Mutex<Option<Sender<Valued>>>,
    /// 消费者端；`TakeEventsRx` 一次性取出，避免双消费者。
    eventsRx: Mutex<Option<Receiver<Valued>>>,
    subscriptionIdleTimeout: Duration,
}

/// 创建订阅器并应用配置；通道在此时建立。
pub fn NewSubscriber(env: Arc<dyn Env>, mut config: Vec<SubscriberConfig>) -> FlushSubscriber {
    let (tx, rx) = mpsc::channel();
    let mut subs = FlushSubscriber {
        env,
        subscriptions: HashMap::new(),
        eventsTunnel: Mutex::new(Some(tx)),
        eventsRx: Mutex::new(Some(rx)),
        subscriptionIdleTimeout,
    };
    for c in &mut config {
        c(&mut subs);
    }
    subs
}

impl FlushSubscriber {
    /// 拉取当前 Store 列表并对齐订阅：新增 / BootAt 变更重建 / 移除陈旧。
    pub fn UpdateStoreTopology(&mut self) -> Result<(), String> {
        let stores = self.env.Stores()?;
        let mut store_set = HashMap::new();
        for store in stores {
            store_set.insert(store.ID, ());
            match self.subscriptions.get(&store.ID) {
                None => {
                    self.addSubscription(&store);
                }
                // BootAt 不同说明 Store 重启，旧订阅失效。
                Some(sub) if sub.storeBootAt != store.BootAt => {
                    self.removeSubscription(store.ID);
                    self.addSubscription(&store);
                }
                Some(_) => {}
            }
        }
        // 拓扑中消失的 store 必须摘掉，防止向已下线节点推送。
        let stale: Vec<u64> = self
            .subscriptions
            .keys()
            .copied()
            .filter(|id| !store_set.contains_key(id))
            .collect();
        for id in stale {
            self.removeSubscription(id);
        }
        Ok(())
    }

    /// 登记订阅并清缓存，迫使后续拨号拿到新连接。
    fn addSubscription(&mut self, store: &Store) {
        let mut subscription = Subscription {
            storeBootAt: store.BootAt,
            pendingError: Arc::new(Mutex::new(None)),
            stop: None,
            background: None,
        };
        self.connect(store.ID, &mut subscription);
        self.subscriptions.insert(store.ID, subscription);
        // 保留 Rust 环境既有契约：新 Store 订阅后失效旧缓存句柄。
        let _ = self.env.ClearCache(store.ID);
    }

    fn removeSubscription(&mut self, id: u64) {
        if let Some(mut subscription) = self.subscriptions.remove(&id) {
            subscription.close();
        }
    }

    /// 清空全部订阅（任务结束或重置场景）。
    pub fn Clear(&mut self) {
        for (_, mut subscription) in self.subscriptions.drain() {
            subscription.close();
        }
    }

    /// 终止订阅器：清理所有 Store 状态并关闭共享事件通道。
    pub fn Drop(&mut self) {
        self.Clear();
        self.eventsTunnel.lock().unwrap().take();
    }

    /// 对所有可重试连接错误清缓存并重新拨号。
    pub fn HandleErrors(&mut self) {
        let ids: Vec<u64> = self.subscriptions.keys().copied().collect();
        for id in ids {
            let Some(mut subscription) = self.subscriptions.remove(&id) else {
                continue;
            };
            let error = subscription.pendingError.lock().unwrap().clone();
            let Some(error) = error else {
                self.subscriptions.insert(id, subscription);
                continue;
            };
            // 对齐 Go：Unimplemented 表示该 Store 不支持订阅，不应重试。
            if error.to_ascii_lowercase().contains("unimplemented") {
                self.subscriptions.insert(id, subscription);
                continue;
            }
            let _ = self.env.ClearCache(id);
            self.connect(id, &mut subscription);
            self.subscriptions.insert(id, subscription);
        }
    }

    /// 聚合当前各 Store 的连接错误；无错误时返回 `Ok(())`。
    pub fn PendingErrors(&self) -> Result<(), String> {
        let errors: Vec<String> = self
            .subscriptions
            .iter()
            .filter_map(|(id, sub)| {
                sub.pendingError
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|error| format!("store {id} has error: {error}"))
            })
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    /// 向事件隧道推送一条 Valued；接收端已关闭则返回错误。
    pub fn PushEvent(&self, v: Valued) -> Result<(), String> {
        self.eventsTunnel
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| "event tunnel is closed".to_string())?
            .send(v)
            .map_err(|e| e.to_string())
    }

    /// 取出接收端所有权；再次调用得到 `None`。
    pub fn TakeEventsRx(&self) -> Option<Receiver<Valued>> {
        self.eventsRx.lock().unwrap().take()
    }

    /// 当前仍登记的 store 订阅数，供拓扑断言使用。
    pub fn SubscriptionCount(&self) -> usize {
        self.subscriptions.len()
    }

    /// 由 flush 区间与时间戳构造推进器可消费的 `Valued`。
    pub fn EventFromFlush(start: &[u8], end: &[u8], ts: u64) -> Valued {
        Valued {
            Key: Span {
                StartKey: start.to_vec(),
                EndKey: end.to_vec(),
            },
            Value: ts,
        }
    }

    fn connect(&self, store_id: u64, subscription: &mut Subscription) {
        subscription.close();
        *subscription.pendingError.lock().unwrap() = None;
        let stream = self
            .env
            .GetLogBackupClient(store_id)
            .and_then(|client| client.SubscribeFlushEvents());
        let rx = match stream {
            Ok(rx) => rx,
            Err(error) => {
                *subscription.pendingError.lock().unwrap() = Some(error);
                return;
            }
        };
        let output = self.eventsTunnel.lock().unwrap().as_ref().cloned();
        let Some(output) = output else { return };
        let (stop_tx, stop_rx) = mpsc::channel();
        let pending = subscription.pendingError.clone();
        let idle_timeout = self.subscriptionIdleTimeout;
        subscription.stop = Some(stop_tx);
        subscription.background = Some(thread::spawn(move || {
            let mut last_activity = Instant::now();
            loop {
                if stop_rx.try_recv().is_ok() {
                    return;
                }
                let poll = if idle_timeout.is_zero() {
                    Duration::from_millis(50)
                } else {
                    idle_timeout.min(Duration::from_millis(50))
                };
                let received = rx.recv_timeout(poll);
                match received {
                    Ok(events) => {
                        last_activity = Instant::now();
                        for event in events {
                            if output
                                .send(Self::EventFromFlush(
                                    &event.StartKey,
                                    &event.EndKey,
                                    event.Checkpoint,
                                ))
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if idle_timeout.is_zero() || last_activity.elapsed() < idle_timeout {
                            continue;
                        }
                        *pending.lock().unwrap() = Some(format!(
                            "flush subscription from store id {store_id} has no activity for {idle_timeout:?}"
                        ));
                        return;
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        *pending.lock().unwrap() = Some(format!(
                            "while receiving from store id {store_id}: stream closed"
                        ));
                        return;
                    }
                }
            }
        }));
    }
}

impl Subscription {
    fn close(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(background) = self.background.take() {
            let _ = background.join();
        }
    }
}
