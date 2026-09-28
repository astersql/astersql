// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 阿里云 OSS 标准重试器实现。
//
// 对应 Go `ossstore/retry.go`：对访问对象存储时的瞬时网络/服务错误做指数退避重试。
// 指数退避指第 n 次重试等待约 `base_delay * 2^(n-1)`，并钳制在 `max_backoff`。
// 阿里云 ECS 实例元数据地址为 `100.100.100.200`，相关错误需单独识别，
// 以免把凭证拉取失败误判为可重试的 OSS 业务错误。

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use anyhow::{Error, Result};

use crate::interface::OssServiceError;

/// 单次操作允许的最大尝试次数（含首次），与 Go 常量对齐。
pub const MAX_ATTEMPTS: i32 = 20;
/// 阿里云 ECS 实例元数据服务地址，用于识别元数据拉取失败。
pub const ECS_META_ADDRESS: &str = "100.100.100.200";

const RETRIABLE_ERROR_STRINGS: [&str; 12] = [
    "connection reset",
    "connection refused",
    "use of closed network connection",
    "unexpected EOF reading trailer",
    "transport connection broken",
    "server closed idle connection",
    "bad record MAC",
    "stream error:",
    "tls: use of closed connection",
    "connection was forcibly closed",
    "broken pipe",
    "crc is inconsistent",
];

/// OSS 场景下的标准重试策略：可重试判定、最大次数与退避延迟。
#[derive(Clone, Debug)]
pub struct OssRetryer {
    /// 最大尝试次数。
    pub max_attempts: i32,
    /// 首次退避基数（通常 1 秒）。
    pub base_delay: Duration,
    /// 退避等待上限，防止指数增长过久。
    pub max_backoff: Duration,
}

impl Default for OssRetryer {
    fn default() -> Self {
        Self {
            max_attempts: MAX_ATTEMPTS,
            base_delay: Duration::from_secs(1),
            max_backoff: Duration::from_secs(32),
        }
    }
}

impl s3like::StandardRetryer for OssRetryer {
    /// 按阿里云 SDK 的 HTTP 状态、服务错误码和网络错误规则判断是否重试。
    fn IsErrorRetryable(&self, error: &Error) -> bool {
        for cause in error.chain() {
            if let Some(service_error) = cause.downcast_ref::<OssServiceError>()
                && matches!(
                    service_error.code.as_str(),
                    "RequestTimeTooSkewed" | "BadRequest"
                )
            {
                return true;
            }

            if let Some(sdk_error) = cause.downcast_ref::<ali_oss_rs::error::Error>() {
                match sdk_error {
                    ali_oss_rs::error::Error::StatusError(status)
                        if is_retryable_status(*status) =>
                    {
                        return true;
                    }
                    ali_oss_rs::error::Error::ApiError(response)
                        if matches!(
                            response.code.as_str(),
                            "RequestTimeTooSkewed" | "BadRequest"
                        ) =>
                    {
                        return true;
                    }
                    _ => {}
                }
            }

            if let Some(request_error) = cause.downcast_ref::<reqwest::Error>()
                && (request_error.is_timeout()
                    || request_error.is_connect()
                    || request_error.status().is_some_and(is_retryable_status))
            {
                return true;
            }

            if let Some(io_error) = cause.downcast_ref::<std::io::Error>()
                && matches!(
                    io_error.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::UnexpectedEof
                )
            {
                return true;
            }
        }

        error.chain().any(|cause| {
            let message = cause.to_string();
            RETRIABLE_ERROR_STRINGS
                .iter()
                .any(|pattern| message.contains(pattern))
        })
    }

    fn MaxAttempts(&self) -> i32 {
        self.max_attempts
    }

    /// Full jitter：`[0, min(base_delay * 2^attempt, max_backoff))`。
    fn RetryDelay(&self, attempt: i32, _: &Error) -> Result<Duration> {
        let exponent = u32::try_from(attempt).unwrap_or(0).min(31);
        let ceiling = self
            .base_delay
            .saturating_mul(1_u32 << exponent)
            .min(self.max_backoff);
        let jitter = random_unit_interval();
        Ok(Duration::from_secs_f64(ceiling.as_secs_f64() * jitter))
    }

    fn GetRetryToken(&self, _: &storeapi::Context, _: &Error) -> Result<s3like::ReleaseToken> {
        // OSS 暂无令牌桶限流；返回空释放回调即可。
        Ok(Box::new(|_| Ok(())))
    }

    fn GetInitialToken(&self) -> s3like::ReleaseToken {
        Box::new(|_| Ok(()))
    }

    /// 错误信息是否涉及 ECS 元数据地址（凭证链路问题）。
    fn IsInstanceMetadataError(&self, error: &Error) -> bool {
        error.to_string().contains(ECS_META_ADDRESS)
    }
}

/// 用标准库每次生成的随机哈希键取得 `[0, 1)` 的 53 位随机小数。
fn random_unit_interval() -> f64 {
    let bits = RandomState::new().build_hasher().finish() >> 11;
    bits as f64 / (1_u64 << 53) as f64
}

fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.as_u16() >= 500
        || matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED
                | reqwest::StatusCode::REQUEST_TIMEOUT
                | reqwest::StatusCode::TOO_MANY_REQUESTS
        )
}

impl OssRetryer {
    /// 转发到 `StandardRetryer` 实现，便于直接调用。
    pub fn IsErrorRetryable(&self, error: &Error) -> bool {
        <Self as s3like::StandardRetryer>::IsErrorRetryable(self, error)
    }

    /// 转发最大尝试次数查询。
    pub fn MaxAttempts(&self) -> i32 {
        <Self as s3like::StandardRetryer>::MaxAttempts(self)
    }

    /// 转发退避延迟计算。
    pub fn RetryDelay(&self, attempt: i32, error: &Error) -> Result<Duration> {
        <Self as s3like::StandardRetryer>::RetryDelay(self, attempt, error)
    }

    /// 转发 ECS 元数据错误判定。
    pub fn IsInstanceMetadataError(&self, error: &Error) -> bool {
        <Self as s3like::StandardRetryer>::IsInstanceMetadataError(self, error)
    }
}

/// 构造包装了默认 `OssRetryer` 的通用 `s3like::Retryer`。
pub fn new_retryer() -> s3like::Retryer {
    s3like::NewRetryer(Box::new(OssRetryer::default()))
}
