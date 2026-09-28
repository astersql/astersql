// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 陈旧读（stale read）错误类型定义。
//
// 陈旧读按历史时间戳读取快照，不参与当前最新提交视图。
// 本模块集中定义解析/校验 AS OF、外部时间戳等路径上可能返回的错误种类与构造辅助。

use std::fmt;

/// 陈旧读错误种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// AS OF TIMESTAMP 表达式解析或求值失败。
    AsOf,
    /// AS OF 时间戳已被求值过，不允许重复求值。
    AlreadyEvaluated,
    /// 当前场景不支持陈旧读。
    Unsupported,
    /// 时间戳非法（例如超出允许范围或格式错误）。
    InvalidTimestamp,
    /// 后端（存储/会话后端）在获取或校验时间戳时失败。
    Backend,
}

/// 陈旧读错误：携带种类与可读消息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// 错误种类。
    pub kind: ErrorKind,
    /// 面向调用方的错误消息。
    pub message: String,
}

impl Error {
    /// 用指定种类与消息构造错误。
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// 构造 AS OF 相关错误。
    pub fn as_of(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::AsOf, message)
    }

    /// 构造后端相关错误。
    pub fn backend(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Backend, message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}
