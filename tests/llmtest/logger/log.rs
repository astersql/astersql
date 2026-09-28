// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Global development logger for llmtest (Go `tests/llmtest/logger/log.go`).

// 本文件对应 `tests/llmtest/logger/log.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
use crate::stubs::zap;
use std::sync::LazyLock;

/// Global is the global logger.
///
/// Mirrors Go `var Global *zap.Logger`, initialized via package `init` with
/// `zap.NewDevelopment()` (panic on error).
// `Global` 记录跨函数共享的固定约束、错误文本或全局状态。
pub static Global: LazyLock<zap::Logger> = LazyLock::new(|| match zap::NewDevelopment() {
    Ok(logger) => logger,
    Err(err) => panic!("{err}"),
});

/// Force initialization of [`Global`] (Go package `init` runs before first use).
// `ensure_init` 承担当前文件中的一段辅助职责或状态转换。
pub fn ensure_init() {
    LazyLock::force(&Global);
}
