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

// 可取消睡眠工具。
//
// 对齐 Go `context` + `time.Timer`：在指定时长内等待，或在取消令牌触发时提前返回。
// 输掉的一边 future 被丢弃，对应 Go 里 `Timer.Stop` 的清理语义。

use std::fmt;
use std::time::Duration;

/// 取消令牌，语义对齐 Go `context.Context` 的取消信号。
pub use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Sleep 提前结束时的错误：仅表示被取消。
pub enum SleepError {
    /// 等待期间取消令牌被触发。
    Cancelled,
}

impl fmt::Display for SleepError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("context canceled")
    }
}

impl std::error::Error for SleepError {}

/// 睡眠 `duration`；若 `context` 先取消则返回 `Cancelled`。
/// Sleeps for `duration`, returning early when `context` is cancelled.
///
/// Dropping the losing `tokio::time::Sleep` future provides the same timer
/// cleanup guarantee as the deferred `Timer.Stop` in the Go implementation.
pub async fn Sleep(context: &CancellationToken, duration: Duration) -> Result<(), SleepError> {
    tokio::select! {
        _ = tokio::time::sleep(duration) => Ok(()),
        _ = context.cancelled() => Err(SleepError::Cancelled),
    }
}
