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

//! Error helpers ported from `br/pkg/backup/prepare_snap/errors.go`.
//!
//! `prepare_snap` 在 Rust 侧不直接依赖 PingCAP 的错误包装库。
//! 因此这里先提供一个足够表达链路语义的本地错误类型。
//! 目标不是复刻完整错误生态，而是保留调用方关心的消息、包裹关系和 EOF 判定。

use crate::env::errorpb;

/// Shared error type for this crate (local stand-in for pingcap/errors).
/// 通过 `message + cause` 保留最小可用的错误链，方便上层追加上下文。
#[derive(Debug, Clone)]
pub struct Error {
    message: String,
    cause: Option<Box<Error>>,
    kind: ErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorKind {
    Message,
    Eof,
}

impl Error {
    /// 构造叶子错误，适用于直接由字符串消息生成的失败。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            cause: None,
            kind: ErrorKind::Message,
        }
    }

    /// 在现有错误外再包一层消息，对应 Go `errors.Annotate`。
    pub fn annotate(err: Self, msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            cause: Some(Box::new(err)),
            kind: ErrorKind::Message,
        }
    }

    /// 格式化版本单独保留接口名，方便和 Go 调用点一一对照。
    pub fn annotatef(err: Self, msg: String) -> Self {
        Self::annotate(err, msg)
    }

    /// 供调用方读取最外层错误消息，而不展开整条错误链。
    pub fn message(&self) -> &str {
        &self.message
    }

    /// `prepare_snap` 需要把流结束当成可识别控制信号，因此单独提供 EOF 判定。
    pub fn is_eof(&self) -> bool {
        self.kind == ErrorKind::Eof
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(cause) = &self.cause {
            write!(f, "{}: {}", self.message, cause)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// convertErr maps `errorpb.Error` to a BR-layer error (nil → None).
/// kvproto 的错误对象在这里被压平成文本，足够支撑当前迁移范围内的分支判断。
pub fn convertErr(err: Option<&errorpb::Error>) -> Option<Error> {
    let Some(err) = err else {
        return None;
    };
    Some(Error::new(err.Message.clone()))
}

/// 返回租约过期错误，供上层把等待快照应用流程判定为需要重试。
pub fn leaseExpired() -> Error {
    Error::new("the lease has expired")
}

/// 标记当前分支尚未支持的操作，保持与 Go 常量文案一致。
pub fn unsupported() -> Error {
    Error::new("unsupported operation")
}

/// 表示重试次数已经耗尽，调用方通常会直接向上返回。
pub fn retryLimitExceeded() -> Error {
    Error::new("the limit of retrying exceeded")
}

/// 为流读取逻辑构造一个可被 `is_eof` 识别的结束错误。
pub fn eof() -> Error {
    Error {
        message: "EOF".to_owned(),
        cause: None,
        kind: ErrorKind::Eof,
    }
}
