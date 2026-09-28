// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// InfoSchema 所需的请求上下文（RequestContext）接口。
//
// 对应 Go 的 `context.Context` 只读表面：截止时间、取消、错误与键值。
// InfoSchema V2 可能惰性加载元数据，并在库之间检查取消；保留完整表面
// 便于调用方传入 deadline / 请求值，而无需把 InfoSchema 耦合到 planner 或 session。

use std::any::Any;
use std::sync::Arc;
use std::time::Instant;

/// Observable terminal states of a Go-compatible request context.
/// 与 Go `context.Context` 兼容的可观察终态：已取消或已超过截止时间。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextError {
    /// 对应 Go 的 `context.Canceled`。
    Cancelled,
    /// 对应 Go 的 `context.DeadlineExceeded`。
    DeadlineExceeded,
}

/// The context surface needed by InfoSchema.
///
/// InfoSchema normally performs in-memory lookups, but the v2 implementation
/// may load schema metadata lazily and checks cancellation between schemas.
/// Keeping the complete read-only Go context surface also lets callers pass
/// deadlines and request values through without coupling InfoSchema to the
/// planner or session crates.
///
/// InfoSchema 需要的上下文接口：通常做内存查找，但 V2 可能惰性加载并在库间检查取消。
pub trait RequestContext: Send + Sync {
    /// 可选截止时间（deadline）。
    fn deadline(&self) -> Option<Instant>;
    /// 是否已完成（取消或超时）。
    fn is_done(&self) -> bool;
    /// 终态错误；未完成时为 None。
    fn error(&self) -> Option<ContextError>;
    /// 按键取请求作用域值（对应 Go `Context.Value`）。
    fn value(&self, key: &(dyn Any + Send + Sync)) -> Option<&(dyn Any + Send + Sync)>;
}

/// 空上下文：永不到期、不取消、无键值；对应 Go 的 Background / TODO 根上下文。
#[derive(Clone, Copy, Debug, Default)]
struct EmptyContext;

impl RequestContext for EmptyContext {
    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn is_done(&self) -> bool {
        false
    }

    fn error(&self) -> Option<ContextError> {
        None
    }

    fn value(&self, _key: &(dyn Any + Send + Sync)) -> Option<&(dyn Any + Send + Sync)> {
        None
    }
}

/// 长生命周期顶层操作使用的 Background 单例。
static BACKGROUND: EmptyContext = EmptyContext;
/// 调用方尚未选定真实上下文时使用的 TODO 单例。
static TODO_CONTEXT: EmptyContext = EmptyContext;

/// Empty root context used by long-lived top-level operations.
/// 长生命周期顶层操作使用的空根上下文（对应 Go `context.Background`）。
pub fn Background() -> &'static dyn RequestContext {
    &BACKGROUND
}

/// 返回可共享的 Background 上下文 `Arc`。
pub fn BackgroundArc() -> Arc<dyn RequestContext> {
    Arc::new(EmptyContext)
}

/// Empty root context used when the caller has not selected a real context.
/// 调用方尚未选定真实上下文时使用的空根上下文（对应 Go `context.TODO`）。
pub fn TODO() -> &'static dyn RequestContext {
    &TODO_CONTEXT
}

/// 返回可共享的 TODO 上下文 `Arc`。
pub fn TODOArc() -> Arc<dyn RequestContext> {
    Arc::new(EmptyContext)
}
