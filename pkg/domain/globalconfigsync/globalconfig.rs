// Copyright 2023 PingCAP, Inc.
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

// 全局配置（Global Config）同步器。
//
// 将 TiDB/AsterSQL 侧变量变更经有界队列通知，并写入 PD（Placement Driver，
// 集群元数据与调度中心）的全局配置存储，实现多实例配置对齐。

use crossbeam_channel::{Receiver, RecvError, Sender, bounded};
use std::sync::Arc;
use thiserror::Error;

/// 内部通知通道容量（与 Go 侧有界队列深度对齐）。
const NOTIFY_CHANNEL_CAPACITY: usize = 8;

/// The event associated with one PD global-configuration entry.
///
/// The discriminants mirror Go `pdpb.EventType`: PUT is zero and DELETE is one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(i32)]
pub enum GlobalConfigEventType {
    #[default]
    Put = 0,
    Delete = 1,
}

/// One PD global-configuration entry.
///
/// PD 全局配置的一条条目，对应 Go `pd.GlobalConfigItem` 的全部字段。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GlobalConfigItem {
    pub event_type: GlobalConfigEventType,
    pub name: String,
    pub value: String,
    pub payload: Vec<u8>,
}

impl GlobalConfigItem {
    /// 构造一条全局配置项。
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            event_type: GlobalConfigEventType::Put,
            name: name.into(),
            value: value.into(),
            payload: Vec::new(),
        }
    }
}

/// Errors returned by the configured global-configuration client.
///
/// 全局配置客户端返回的错误类型。
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum GlobalConfigError {
    #[error("global configuration client error: {0}")]
    Client(String),
}

/// The narrow part of the PD client used by [`GlobalConfigSyncer`].
///
/// Keeping this as an interface mirrors Go's `pd.Client` dependency while
/// allowing the package integration task to adapt the concrete PD client.
///
/// [`GlobalConfigSyncer`] 所用 PD 客户端的窄接口，对应 Go `pd.Client` 依赖。
pub trait GlobalConfigClient: Send + Sync {
    /// 按 prefix 批量写入全局配置项。
    fn store_global_config(
        &self,
        prefix: &str,
        items: &[GlobalConfigItem],
    ) -> Result<(), GlobalConfigError>;
}

/// Synchronizes configuration entries through a bounded internal queue.
///
/// 经有界内部队列同步配置项到 PD。
pub struct GlobalConfigSyncer {
    client: Option<Arc<dyn GlobalConfigClient>>,
    notify_tx: Sender<GlobalConfigItem>,
    notify_rx: Receiver<GlobalConfigItem>,
}

impl GlobalConfigSyncer {
    /// Creates a syncer. `None` preserves the Go nil-client no-op behavior.
    ///
    /// 创建同步器；`client` 为 `None` 时写入为空操作（对齐 Go nil client）。
    pub fn new(client: Option<Arc<dyn GlobalConfigClient>>) -> Self {
        let (notify_tx, notify_rx) = bounded(NOTIFY_CHANNEL_CAPACITY);
        Self {
            client,
            notify_tx,
            notify_rx,
        }
    }

    /// Stores exactly one item using PD's empty global-config prefix.
    ///
    /// 使用空 prefix 向 PD 写入恰好一项全局配置。
    pub fn store_global_config(&self, item: GlobalConfigItem) -> Result<(), GlobalConfigError> {
        let Some(client) = &self.client else {
            return Ok(());
        };

        client.store_global_config("", std::slice::from_ref(&item))?;
        log::info!(
            "store global config: name={}, value={}",
            item.name,
            item.value
        );
        Ok(())
    }

    /// Pushes an item into the internal bounded queue, blocking when it is full.
    ///
    /// 将配置项推入有界队列；队列满时阻塞发送方。
    pub fn notify(&self, item: GlobalConfigItem) {
        self.notify_tx
            .send(item)
            .expect("global configuration notification receiver is owned by the syncer");
    }

    /// Receives the next queued item in FIFO order.
    ///
    /// 按 FIFO 顺序取出下一条排队通知。
    pub fn recv_notification(&self) -> Result<GlobalConfigItem, RecvError> {
        self.notify_rx.recv()
    }

    /// 返回通知通道容量。
    pub fn notify_capacity(&self) -> usize {
        self.notify_rx.capacity().unwrap_or(0)
    }
}
