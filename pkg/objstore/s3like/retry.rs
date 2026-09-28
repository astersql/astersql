// Copyright 2026 PingCAP, Inc.
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

// S3-like 对象存储客户端的重试策略封装。
//
// 在底层 `StandardRetryer` 之上叠加连接复位、HTTP/2 中断、实例元数据超时等特殊判定，
// 供备份/导入等路径在访问兼容 S3 的对象存储时决定是否重试与退避多久。

#![allow(non_snake_case)]

use std::time::Duration;

use anyhow::{Error, Result, anyhow};

use crate::RecordRetryableError;

/// 重试令牌释放回调：操作结束后调用，可携带可选错误以反馈配额。
pub type ReleaseToken = Box<dyn FnOnce(Option<&Error>) -> Result<()> + Send>;

/// 标准重试器接口，对应 AWS SDK StandardRetryer 的可注入抽象。
pub trait StandardRetryer: Send + Sync {
    /// 判断错误是否可重试。
    fn IsErrorRetryable(&self, err: &Error) -> bool;
    /// 最大尝试次数（含首次）。
    fn MaxAttempts(&self) -> i32;
    /// 第 `attempt` 次失败后的退避延迟。
    fn RetryDelay(&self, attempt: i32, err: &Error) -> Result<Duration>;
    /// 获取后续重试所需的令牌（用于限流）。
    fn GetRetryToken(&self, ctx: &storeapi::Context, opErr: &Error) -> Result<ReleaseToken>;
    /// 获取首次请求前的初始令牌。
    fn GetInitialToken(&self) -> ReleaseToken;
    /// 是否为访问实例元数据服务（IMDS）相关错误。
    fn IsInstanceMetadataError(&self, err: &Error) -> bool;
}

/// 包装 `StandardRetryer`，注入对象存储场景下的额外可重试判定。
pub struct Retryer {
    standardRetryer: Box<dyn StandardRetryer>,
}

/// 由底层标准重试器构造带对象存储特化逻辑的 `Retryer`。
pub fn NewRetryer(inner: Box<dyn StandardRetryer>) -> Retryer {
    Retryer {
        standardRetryer: inner,
    }
}

impl Retryer {
    /// 综合 failpoint 注入、元数据超时、连接复位/拒绝与 HTTP/2 中断判定是否可重试。
    pub fn IsErrorRetryable(&self, err: &Error) -> bool {
        // 测试用 failpoint：把任意错误替换为「连接被对端重置」消息。
        let injected = fail::eval("replace-error-to-connection-reset-by-peer", |_| {
            "read tcp *.*.*.*:*->*.*.*.*:*: read: connection reset by peer".to_owned()
        });
        let injected_error;
        let effective = if let Some(message) = injected {
            injected_error = anyhow!(message);
            &injected_error
        } else {
            err
        };

        // 实例元数据超时/复位不可重试（避免长时间卡在 IMDS）；
        // 普通连接复位与 HTTP/2 强制关闭可重试；连接拒绝则直接失败。
        let retryable = if self.standardRetryer.IsInstanceMetadataError(effective)
            && (IsDeadlineExceedError(effective) || isConnectionResetError(effective))
        {
            false
        } else if isConnectionResetError(effective) {
            true
        } else if isConnectionRefusedError(effective) {
            false
        } else if IsHTTP2ConnAborted(effective) {
            true
        } else {
            self.standardRetryer.IsErrorRetryable(effective)
        };
        if retryable {
            RecordRetryableError(&effective.to_string());
        }
        retryable
    }

    /// 转发底层最大尝试次数。
    pub fn MaxAttempts(&self) -> i32 {
        self.standardRetryer.MaxAttempts()
    }

    /// 计算退避延迟，并保证至少 1 秒，避免过于频繁的重试冲击对象存储。
    pub fn RetryDelay(&self, attempt: i32, err: &Error) -> Result<Duration> {
        Ok(self
            .standardRetryer
            .RetryDelay(attempt, err)?
            .max(Duration::from_secs(1)))
    }

    /// 转发获取重试令牌。
    pub fn GetRetryToken(&self, ctx: &storeapi::Context, opErr: &Error) -> Result<ReleaseToken> {
        self.standardRetryer.GetRetryToken(ctx, opErr)
    }

    /// 转发获取初始令牌。
    pub fn GetInitialToken(&self) -> ReleaseToken {
        self.standardRetryer.GetInitialToken()
    }
}

/// 上下文截止时间（deadline）已超过。
pub fn IsDeadlineExceedError(err: &Error) -> bool {
    err.to_string().contains("context deadline exceeded")
}

/// 读操作遇到「connection reset」（连接被对端重置）。
pub(crate) fn isConnectionResetError(err: &Error) -> bool {
    err.to_string().contains("read: connection reset")
}

/// 连接被拒绝（对端未监听或防火墙拦截）。
fn isConnectionRefusedError(err: &Error) -> bool {
    err.to_string().contains("connection refused")
}

/// HTTP/2 连接被客户端强制关闭、服务端 GOAWAY 或意外 EOF。
pub fn IsHTTP2ConnAborted(err: &Error) -> bool {
    [
        "http2: client connection force closed via ClientConn.Close",
        "http2: server sent GOAWAY and closed the connection",
        "unexpected EOF",
    ]
    .iter()
    .any(|pattern| err.to_string().contains(pattern))
}
