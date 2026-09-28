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

// DDL 事件处理入口。
//
// 维护有界 DDL 事件队列，并将 schema 变更交给 `Subscriber` 更新统计信息。
// 对应 Go 侧 StatsHandle 对 DDL 通知的入队与消费路径。

use std::collections::VecDeque;

use crate::subscriber::{Error, SchemaChangeEvent, StatsBackend, Subscriber};

/// DDL 事件队列容量上限（与 Go `DDLEventChannelCapacity` 对齐）。
pub const DDL_EVENT_CHANNEL_CAPACITY: usize = 1_000;

/// DDL 处理器：持有待处理事件队列与统计订阅者。
pub struct DdlHandler<B> {
    /// 待消费的 schema 变更事件（FIFO）。
    events: VecDeque<SchemaChangeEvent>,
    /// 实际执行统计写入的订阅者。
    subscriber: Subscriber<B>,
}

impl<B: StatsBackend> DdlHandler<B> {
    /// 用给定统计后端构造处理器，预分配事件队列容量。
    pub fn new(backend: B) -> Self {
        Self {
            events: VecDeque::with_capacity(DDL_EVENT_CHANNEL_CAPACITY),
            subscriber: Subscriber::new(backend),
        }
    }

    /// 将事件入队；队列已满时返回错误，避免无限堆积。
    pub fn enqueue(&mut self, event: SchemaChangeEvent) -> Result<(), Error> {
        if self.events.len() == DDL_EVENT_CHANNEL_CAPACITY {
            return Err(Error("DDL event channel is full".into()));
        }
        self.events.push_back(event);
        Ok(())
    }

    /// 弹出队首事件；队列为空时返回 `None`。
    pub fn next_event(&mut self) -> Option<SchemaChangeEvent> {
        self.events.pop_front()
    }

    /// DDL statistics updates are currently best effort in Go. The subscriber
    /// error is reported to the backend and deliberately does not escape.
    ///
    /// 处理单个 DDL 事件：订阅者失败时仅告警并吞掉错误（与 Go best-effort 一致）。
    pub fn handle_ddl_event(&mut self, event: &SchemaChangeEvent) -> Result<(), Error> {
        if let Err(error) = self.subscriber.handle(event) {
            self.subscriber
                .backend_mut()
                .warn_ignored_event_error(event, &error);
        }
        Ok(())
    }

    /// 只读访问内部订阅者（测试断言用）。
    pub fn subscriber(&self) -> &Subscriber<B> {
        &self.subscriber
    }

    /// 可变访问内部订阅者。
    pub fn subscriber_mut(&mut self) -> &mut Subscriber<B> {
        &mut self.subscriber
    }
}

/// 测试专用包装：转发到 `update_stats_with_count_delta_and_modify_count_delta`。
pub fn update_stats_with_count_delta_and_modify_count_delta_for_test(
    backend: &mut impl StatsBackend,
    table_id: i64,
    count_delta: i64,
    modify_count_delta: i64,
) -> Result<(), Error> {
    crate::subscriber::update_stats_with_count_delta_and_modify_count_delta(
        backend,
        table_id,
        count_delta,
        modify_count_delta,
    )
}
