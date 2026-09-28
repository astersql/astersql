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

// TTL 通知通道：在 etcd 前缀下发布/订阅轻量事件。
//
// 通知与命令通道分离：通知是单向广播（无 request/response 配对），
// 用于告知其他节点状态变化；键前缀为 `/tidb/ttl/notification/`。

use crate::command::{
    ClientContext, ClientError, EtcdClient, EtcdEventKind, EtcdStore, MockClient,
};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

/// etcd 中 TTL 通知键的公共前缀，后接 notification_type。
pub const TTL_NOTIFICATION_PREFIX: &str = "/tidb/ttl/notification/";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一条通知事件的载荷（UTF-8 文本）。
pub struct NotificationEvent {
    /// 通知数据正文。
    pub data: String,
}

/// 通知订阅端的接收句柄，封装跨线程 channel。
pub struct NotificationReceiver {
    receiver: mpsc::Receiver<NotificationEvent>,
}

impl NotificationReceiver {
    /// 阻塞接收下一条通知。
    pub fn recv(&self) -> Result<NotificationEvent, mpsc::RecvError> {
        self.receiver.recv()
    }

    /// 在超时时间内等待下一条通知。
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<NotificationEvent, mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    /// 非阻塞尝试接收；无数据时立即返回。
    pub fn try_recv(&self) -> Result<NotificationEvent, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
}

/// 通知客户端：发布通知并按类型订阅。
pub trait NotificationClient: Send + Sync {
    /// 向指定 notification_type 写入一条通知（覆盖同键最新值）。
    fn notify(
        &self,
        ctx: &ClientContext,
        notification_type: &str,
        data: &str,
    ) -> Result<(), ClientError>;
    /// 订阅指定类型的通知流，返回可接收事件的句柄。
    fn watch_notification(
        &self,
        ctx: ClientContext,
        notification_type: &str,
    ) -> NotificationReceiver;
}

/// 基于 etcd store 构造真实通知客户端。
pub fn new_notification_client(store: Arc<EtcdStore>) -> Arc<dyn NotificationClient> {
    Arc::new(EtcdClient::new(store))
}

impl NotificationClient for EtcdClient {
    fn notify(
        &self,
        ctx: &ClientContext,
        notification_type: &str,
        data: &str,
    ) -> Result<(), ClientError> {
        self.store.put(
            ctx,
            format!("{TTL_NOTIFICATION_PREFIX}{notification_type}"),
            data.as_bytes().to_vec(),
            Some(1),
        )
    }

    fn watch_notification(
        &self,
        ctx: ClientContext,
        notification_type: &str,
    ) -> NotificationReceiver {
        let events = self.store.watch(
            ctx.clone(),
            format!("{TTL_NOTIFICATION_PREFIX}{notification_type}"),
            false,
        );
        let (sender, receiver) = mpsc::channel();
        // 后台线程把 etcd Put 事件转成 NotificationEvent；Delete 忽略。
        thread::spawn(move || {
            while ctx.error().is_none() {
                match events.recv_timeout(Duration::from_millis(20)) {
                    Ok(event) if event.kind == EtcdEventKind::Put => {
                        if sender
                            .send(NotificationEvent {
                                data: String::from_utf8_lossy(&event.value).into_owned(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        NotificationReceiver { receiver }
    }
}

/// 构造内存 mock 通知客户端，供单测不依赖真实 etcd。
pub fn new_mock_notification_client() -> Arc<MockClient> {
    Arc::new(MockClient::new())
}

impl NotificationClient for MockClient {
    fn notify(
        &self,
        ctx: &ClientContext,
        notification_type: &str,
        _data: &str,
    ) -> Result<(), ClientError> {
        let mut state = self.state().lock().unwrap();
        let Some(watchers) = state.notification_watchers.get_mut(notification_type) else {
            return Ok(());
        };
        let event = NotificationEvent::default();
        // 同步尝试投递；若某个 watcher 通道满，剩余改为异步重试。
        let mut unsent = Vec::new();
        for (index, watcher) in watchers.iter().enumerate() {
            if let Some(err) = ctx.error() {
                return Err(err);
            }
            match watcher.sender.try_send(event.clone()) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(_)) => {
                    unsent.extend(
                        watchers[index..]
                            .iter()
                            .map(|watcher| watcher.sender.clone()),
                    );
                    break;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {}
            }
        }
        drop(state);
        if !unsent.is_empty() {
            let ctx = ctx.clone();
            thread::spawn(move || {
                for sender in unsent {
                    while ctx.error().is_none() {
                        match sender.try_send(event.clone()) {
                            Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => break,
                            Err(mpsc::TrySendError::Full(_)) => {
                                thread::sleep(Duration::from_millis(10));
                            }
                        }
                    }
                }
            });
        }
        Ok(())
    }

    fn watch_notification(
        &self,
        ctx: ClientContext,
        notification_type: &str,
    ) -> NotificationReceiver {
        let (sender, receiver) = mpsc::sync_channel(8);
        let id = self.next_watcher_id();
        self.state()
            .lock()
            .unwrap()
            .notification_watchers
            .entry(notification_type.to_owned())
            .or_default()
            .push(crate::command::MockNotificationWatcher { id, sender });
        // The Go mock deliberately keeps notification watchers for its lifetime;
        // unlike command watchers it does not unregister them on context cancel.
        let _ = ctx;
        NotificationReceiver { receiver }
    }
}
