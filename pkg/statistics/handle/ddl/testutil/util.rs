// Copyright 2024 PingCAP, Inc.
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

// DDL 测试辅助：事务包装处理与事件通道查找。
//
// 通过真实 `call_with_sctx(FLAG_WRAP_TXN)` 包装 DDL 处理，并从事件通道
// 中按 `ActionType` 阻塞/超时查找下一条匹配事件。

use aster_sql_ddl_notifier::SchemaChangeEvent;
use aster_sql_kv::{Context, InternalDDLNotifier, WithInternalSourceType};
use aster_sql_meta_model::ActionType;
use aster_sql_statistics_handle_util::{
    FLAG_WRAP_TXN, SessionContext, SessionPool, StatsError, call_with_sctx,
};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// `statistics::handle::Handle` 向测试 helper 暴露的生产能力。
///
/// Rust 主 Handle 完整接线时只需实现此边界；事务开启、提交和回滚由本 helper
/// 强制走生产 `call_with_sctx`，调用方不能用自造闭包绕开 `FLAG_WRAP_TXN`。
pub trait TransactionalDDLHandle {
    /// 返回 statistics Handle 的真实会话池（对应 Go `h.SPool()`）。
    fn SPool(&self) -> &dyn SessionPool;

    /// 在给定请求/会话上下文中处理一条 schema 变更事件。
    fn HandleDDLEvent(
        &self,
        context: &Context,
        session_context: &dyn SessionContext,
        event: &SchemaChangeEvent,
    ) -> std::result::Result<(), StatsError>;

    /// 返回 DDL 事件接收端（`Mutex` 保护以匹配 Go 通道访问）。
    fn DDLEventCh(&self) -> &Mutex<Receiver<SchemaChangeEvent>>;
}

/// 在会话事务中处理给定 DDL 事件。
pub fn HandleDDLEventWithTxn<H: TransactionalDDLHandle>(
    handle: &H,
    event: &SchemaChangeEvent,
) -> std::result::Result<(), StatsError> {
    call_with_sctx(
        handle.SPool(),
        |session_context| {
            let context = WithInternalSourceType(Context::default(), InternalDDLNotifier);
            handle.HandleDDLEvent(&context, session_context, event)
        },
        &[FLAG_WRAP_TXN],
    )
}

/// 从事件通道取出下一条事件，并在事务中处理。
pub fn HandleNextDDLEventWithTxn<H: TransactionalDDLHandle>(
    handle: &H,
) -> std::result::Result<(), StatsError> {
    let event = handle
        .DDLEventCh()
        .lock()
        .expect("DDL event receiver lock is poisoned")
        .recv()
        .expect("DDL event channel is closed");
    HandleDDLEventWithTxn(handle, &event)
}

/// 阻塞接收直到遇到指定 `ActionType` 的事件；通道关闭则 panic。
pub fn FindEvent(
    event_channel: &Receiver<SchemaChangeEvent>,
    event_type: ActionType,
) -> SchemaChangeEvent {
    loop {
        match event_channel.recv() {
            Ok(event) if event.GetType() == event_type => return event,
            Ok(_) => {}
            Err(_) => panic!("DDL event channel is closed"),
        }
    }
}

/// 在全局超时内查找指定类型事件；超时返回 `None`，通道关闭则 panic。
pub fn FindEventWithTimeout(
    event_channel: &Receiver<SchemaChangeEvent>,
    event_type: ActionType,
    timeout_seconds: isize,
) -> Option<SchemaChangeEvent> {
    assert!(timeout_seconds > 0, "non-positive interval for NewTicker");
    let deadline = Instant::now() + Duration::from_secs(timeout_seconds as u64);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match event_channel.recv_timeout(remaining) {
            Ok(event) if event.GetType() == event_type => return Some(event),
            // 非目标事件且未超时：继续等待。
            Ok(_) if Instant::now() < deadline => {}
            Ok(_) | Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => panic!("DDL event channel is closed"),
        }
    }
}
