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

// S3 标准重试器（retryer）实现。
//
// 对应 Go `s3store/retry.go`：在访问对象存储时对瞬时网络/服务错误做指数退避重试。
// 指数退避指第 n 次重试等待约 2^n 秒，并设上限，避免打爆远端。
// EC2 实例元数据（IMDS，链路本地地址 169.254.169.254）相关错误需单独识别，
// 以免把凭证拉取失败误判为可重试的 S3 业务错误。

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use anyhow::{Error, Result};

/// 单次操作允许的最大尝试次数（含首次），与 Go 常量对齐。
pub const MAX_ATTEMPTS: i32 = 20;
/// EC2 实例元数据服务（IMDS）的链路本地地址，用于识别元数据拉取失败。
pub const EC2_META_ADDRESS: &str = "169.254.169.254";
/// 退避等待上限：防止指数增长导致等待过久。
const MAX_BACKOFF: Duration = Duration::from_secs(32);

/// 构造包装了 `S3StandardRetryer` 的通用 `s3like::Retryer`。
pub fn newRetryer() -> s3like::Retryer {
    s3like::NewRetryer(Box::new(S3StandardRetryer))
}

/// S3 场景下的标准重试策略：可重试错误判定、最大次数与退避延迟。
#[derive(Clone, Copy, Debug, Default)]
pub struct S3StandardRetryer;

#[derive(Debug)]
struct InstanceMetadataRetryClassifier;

impl S3StandardRetryer {
    /// 判断错误信息是否涉及 EC2 实例元数据地址（凭证链路问题，而非普通 S3 瞬时故障）。
    pub fn IsInstanceMetadataError(&self, error: &Error) -> bool {
        error.to_string().contains(EC2_META_ADDRESS)
    }
}

impl s3like::StandardRetryer for S3StandardRetryer {
    /// 根据错误消息关键字判断是否值得重试（超时、连接重置、SlowDown 等）。
    fn IsErrorRetryable(&self, error: &Error) -> bool {
        let message = error.to_string().to_ascii_lowercase();
        // 与 Go 侧关键字列表对齐：覆盖网络瞬时故障与 S3 限流/内部错误码。
        [
            "timeout",
            "context deadline exceeded",
            "temporarily unavailable",
            "connection reset",
            "unexpected eof",
            "slowdown",
            "requesttimeout",
            "internalerror",
            "serviceunavailable",
        ]
        .iter()
        .any(|pattern| message.contains(pattern))
    }

    fn MaxAttempts(&self) -> i32 {
        MAX_ATTEMPTS
    }

    /// 按 AWS Go SDK 规则计算指数退避：到达上限前使用 full jitter，之后固定为上限。
    fn RetryDelay(&self, attempt: i32, _error: &Error) -> Result<Duration> {
        // Go SDK precomputes log2(maxBackoff / 1s), and skips jitter once the
        // attempt is greater than that boundary.
        if attempt > 5 {
            return Ok(MAX_BACKOFF);
        }

        let exponent = u32::try_from(attempt).unwrap_or(0);
        let ceiling = Duration::from_secs(1_u64 << exponent);
        Ok(ceiling.mul_f64(random_unit_interval()))
    }

    // The Go implementation explicitly disables the shared token bucket. A
    // no-op release token therefore remains valid regardless of request count.
    // Go 实现关闭了共享令牌桶限流；此处返回空操作的释放令牌，请求数再多也不耗尽。
    fn GetRetryToken(
        &self,
        _ctx: &storeapi::Context,
        _error: &Error,
    ) -> Result<s3like::ReleaseToken> {
        Ok(Box::new(|_| Ok(())))
    }

    fn GetInitialToken(&self) -> s3like::ReleaseToken {
        Box::new(|_| Ok(()))
    }

    fn IsInstanceMetadataError(&self, error: &Error) -> bool {
        S3StandardRetryer::IsInstanceMetadataError(self, error)
    }
}

/// Generate a fresh 53-bit random fraction in `[0, 1)`, matching the range
/// consumed by the AWS Go SDK's exponential jitter implementation.
fn random_unit_interval() -> f64 {
    let bits = RandomState::new().build_hasher().finish() >> 11;
    bits as f64 / (1_u64 << 53) as f64
}

impl storeapi::Retryer for S3StandardRetryer {
    fn retry_config(&self) -> aws_sdk_s3::config::retry::RetryConfig {
        aws_sdk_s3::config::retry::RetryConfig::standard()
            .with_max_attempts(MAX_ATTEMPTS as u32)
            .with_initial_backoff(Duration::from_secs(1))
            .with_max_backoff(MAX_BACKOFF)
    }

    fn retry_classifier(
        &self,
    ) -> Option<storeapi::aws_smithy_runtime_api::client::retries::classifiers::SharedRetryClassifier>
    {
        Some(
            storeapi::aws_smithy_runtime_api::client::retries::classifiers::SharedRetryClassifier::new(
                InstanceMetadataRetryClassifier,
            ),
        )
    }
}

impl storeapi::aws_smithy_runtime_api::client::retries::classifiers::ClassifyRetry
    for InstanceMetadataRetryClassifier
{
    fn classify_retry(
        &self,
        context: &storeapi::aws_smithy_runtime_api::client::interceptors::context::InterceptorContext,
    ) -> storeapi::aws_smithy_runtime_api::client::retries::classifiers::RetryAction {
        match context.output_or_error() {
            Some(Err(error)) if error.to_string().contains(EC2_META_ADDRESS) => {
                storeapi::aws_smithy_runtime_api::client::retries::classifiers::RetryAction::RetryForbidden
            }
            _ => storeapi::aws_smithy_runtime_api::client::retries::classifiers::RetryAction::NoActionIndicated,
        }
    }

    fn name(&self) -> &'static str {
        "TiDB EC2 instance metadata retry classifier"
    }

    fn priority(
        &self,
    ) -> storeapi::aws_smithy_runtime_api::client::retries::classifiers::RetryClassifierPriority
    {
        use storeapi::aws_smithy_runtime_api::client::retries::classifiers::RetryClassifierPriority;
        RetryClassifierPriority::run_after(RetryClassifierPriority::transient_error_classifier())
    }
}
