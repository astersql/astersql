// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Contextual logger helpers ported from `br/pkg/logutil/context.go`.
//!
//! 上下文日志辅助：对齐 Go `br/pkg/logutil/context.go`。
//! Rust 侧用轻量 `Context` 承载可选 Logger，而非 std `context.Context` 的 WithValue。
//! 解析顺序为：上下文内 logger → 包级全局 logger → `default_logger()`。
//! 已包装的 Context 在 `ResetGlobalLogger` 后仍保留旧 logger，与 Go 注释一致。

use std::sync::{LazyLock, RwLock};

use crate::logging::{Field, Logger, default_logger};

// 包级全局 logger；故意不直接绑死 default_logger，以便测试可替换且能看到更新。
static GLOBAL_LOGGER: LazyLock<RwLock<Option<Logger>>> = LazyLock::new(|| RwLock::new(None));

/// Resets the package-global logger used when a context carries no logger.
/// 重置无上下文 logger 时的回退目标；已有 Context 不受影响（对齐 Go）。
pub fn ResetGlobalLogger(l: Option<Logger>) {
    *GLOBAL_LOGGER.write().expect("global logger lock poisoned") = l;
}

// 读锁取出当前全局 logger；poison 时与写路径一致直接 panic。
fn global_logger() -> Option<Logger> {
    GLOBAL_LOGGER
        .read()
        .expect("global logger lock poisoned")
        .clone()
}

/// Lightweight context carrying an optional contextual logger.
/// 轻量上下文：仅携带可选 Logger，对应 Go 的 context.Value(keyLogger)。
#[derive(Clone, Default, Debug)]
pub struct Context {
    logger: Option<Logger>,
}

impl Context {
    /// 空上下文入口，等价于 Go 侧未绑定 logger 的 background context。
    pub fn Background() -> Self {
        Self::default()
    }
}

/// Wraps a context with a logger enriched by additional fields.
/// 在既有上下文 logger（或回退 logger）上叠加字段，返回新 Context。
pub fn ContextWithField(c: Context, fields: impl IntoIterator<Item = Field>) -> Context {
    let logger = LoggerFromContext(&c).With(fields);
    Context {
        logger: Some(logger),
    }
}

/// Returns the contextual logger, falling back to the global logger and then the default logger.
/// 三级回退：Context → GLOBAL_LOGGER → default_logger，与 Go LoggerFromContext 一致。
pub fn LoggerFromContext(c: &Context) -> Logger {
    if let Some(logger) = &c.logger {
        return logger.clone();
    }
    global_logger().unwrap_or_else(default_logger)
}

/// Shorthand for [`LoggerFromContext`].
/// `LoggerFromContext` 的缩写，对齐 Go 的 `CL`。
pub fn CL(c: &Context) -> Logger {
    LoggerFromContext(c)
}
