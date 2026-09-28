// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 事务事件信封：动态载荷与类型标签。
//
// 由 `trx_events.go` 迁移。`TransactionEvent` 用 `eventType` + `Any` 载荷表达
// 多种事务事件；当前仅实现 CopMeetLock（coprocessor 读遇到锁）。

use std::any::Any;

use tikv_client_proto::kvrpcpb::LockInfo as ProtoLockInfo;

/// 事务事件类型标签（对应 Go `type EventType = int`）。
// EventType represents the type of a transaction event.
// EventType 对应 Go 里的 `type EventType = int`，用于给 TransactionEvent 的动态载荷打标签。
// Go 的 int 是平台相关宽度；这里用 isize 作机械占位，后续若接入真实 crate 再统一类型。
pub type EventType = isize;

/// CopMeetLock 事件的类型常量（Go iota 首值）。
// EventTypeCopMeetLock stands for the CopMeetLock event type.
// EventTypeCopMeetLock 对应 Go const iota 的第一个值，用来标记 CopMeetLock 事件。
pub const EventTypeCopMeetLock: EventType = 0;

/// coprocessor 读取遇到锁时产生的事务事件。
// CopMeetLock represents an event that coprocessor reading encounters lock.
// CopMeetLock 对应 Go 结构体，表示 coprocessor 读取过程中遇到锁的事务事件。
pub struct CopMeetLock {
    // LockInfo 保留 Go 字段名和指针语义；Option 表示 Go 的 `*kvrpcpb.LockInfo` 可以为 nil。
    /// 锁信息指针；None 表示 Go 的 nil `*LockInfo`。
    pub LockInfo: Option<Box<ProtoLockInfo>>,
}

/// 可承载任意类型事务事件的信封。
// TransactionEvent represents a transaction event that may belong to any of the possible types.
// TransactionEvent 对应 Go 结构体：inner 保存任意事件载荷，eventType 保存与载荷匹配的事件类型标签。
pub struct TransactionEvent {
    // inner 对应 Go 的 `any` 字段。
    inner: Option<Box<dyn Any>>,
    // eventType 保留 Go 的私有字段语义，用于 GetCopMeetLock 判断 inner 是否应按 CopMeetLock 解释。
    eventType: EventType,
}

impl TransactionEvent {
    /// 若本事件为 CopMeetLock，取出内部载荷；否则返回 None。
    // GetCopMeetLock tries to extract the inner CopMeetLock event from a TransactionEvent. Returns nil if it's not a
    // CopMeetLock event.
    // GetCopMeetLock 对应 Go 值接收者方法：当事件类型为 CopMeetLock 时，尝试把 inner 取回为 CopMeetLock。
    // Rust 没有 Go 的 nil 指针返回值，这里用 Option<&CopMeetLock> 表示成功取到或返回 nil 的两种情况。
    pub fn GetCopMeetLock(&self) -> Option<&CopMeetLock> {
        // 保留 Go 的首要分支：只有标签匹配 EventTypeCopMeetLock 时才访问动态载荷。
        if self.eventType == EventTypeCopMeetLock {
            // Go 代码使用 `e.inner.(*CopMeetLock)`，若标签和实际载荷不一致会触发类型断言 panic；
            // downcast_ref 对应 Go 的类型断言；typed nil 对应为空时返回 None。
            return self
                .inner
                .as_deref()
                .and_then(|inner| inner.downcast_ref::<CopMeetLock>());
        }

        // 对应 Go 的 `return nil`：非 CopMeetLock 事件不暴露任何载荷。
        None
    }
}

/// 将 CopMeetLock 包装为带类型标签的 TransactionEvent。
// WrapCopMeetLock wraps a CopMeetLock event into a TransactionEvent object.
// WrapCopMeetLock 对应 Go 函数：把 CopMeetLock 指针包装进 TransactionEvent，并设置事件类型标签。
pub fn WrapCopMeetLock(copMeetLock: Option<Box<CopMeetLock>>) -> TransactionEvent {
    TransactionEvent {
        // 保持 Go 结构体字面量字段顺序：先写 eventType，再写 inner。
        eventType: EventTypeCopMeetLock,
        // Go 把 `*CopMeetLock` 放入 any；这里把 Box<CopMeetLock> 转成 Box<dyn Any>。
        // 如果 Go 调用方传入 nil，Rust 用 None 表示。
        inner: copMeetLock.map(|event| event as Box<dyn Any>),
    }
}

/// 处理 TransactionEvent 的回调类型别名。
// EventCallback is the callback type that handles `TransactionEvent`s.
// EventCallback 对应 Go 的函数类型别名 `func(event TransactionEvent)`，用于处理事务事件回调。
// Go 函数值可以为 nil 且可闭包捕获环境；这里用 Box<dyn Fn> 表示可捕获闭包，nil 语义需由调用方再包 Option。
pub type EventCallback = Box<dyn Fn(TransactionEvent)>;
