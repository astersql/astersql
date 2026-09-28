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

// Timer API 错误类型定义。
//
// 提供定时器（Timer）存储与客户端操作的统一错误枚举，以及常用错误常量别名。

use thiserror::Error;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
/// 定时器相关错误。
///
/// 覆盖不存在、已存在、版本/事件 ID 不匹配，以及携带自定义消息的通用错误。
pub enum TimerError {
    #[error("timer not exist")]
    /// 指定的定时器不存在。
    TimerNotExist,
    #[error("timer already exists")]
    /// 同命名空间下已存在相同 Key 的定时器。
    TimerExists,
    #[error("timer version not match")]
    /// 乐观并发控制：期望版本与当前记录版本不一致。
    VersionNotMatch,
    #[error("timer event id not match")]
    /// 关闭事件时传入的 EventID 与记录中的不一致。
    EventIdNotMatch,
    #[error("{0}")]
    /// 携带自定义说明的通用错误。
    Message(String),
}

impl TimerError {
    /// 构造带自定义消息的 `Message` 变体。
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

/// 定时器 API 的统一 `Result` 别名。
pub type TimerResult<T> = Result<T, TimerError>;

/// 定时器不存在错误常量。
pub const ErrTimerNotExist: TimerError = TimerError::TimerNotExist;
/// 定时器已存在错误常量。
pub const ErrTimerExists: TimerError = TimerError::TimerExists;
/// 版本不匹配错误常量。
pub const ErrVersionNotMatch: TimerError = TimerError::VersionNotMatch;
/// 事件 ID 不匹配错误常量。
pub const ErrEventIDNotMatch: TimerError = TimerError::EventIdNotMatch;
