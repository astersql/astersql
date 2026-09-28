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

// `retry` 模块单元测试：验证令牌不耗尽、退避总时长区间，以及元数据错误与普通超时的可重试性区分。

use std::time::Duration;

use anyhow::anyhow;

/// 反复申请重试令牌，确认关闭令牌桶后不会因请求次数耗尽而失败。
#[test]
fn test_s3_tidb_retryer_never_exhaust_tokens() {
    let retry = s3store::newRetryer();
    let ctx = storeapi::Context::default();
    let timeout = anyhow!("dns lookup timeout");
    for _ in 0..10_000 {
        let _release = retry.GetRetryToken(&ctx, &timeout).unwrap();
    }
}

/// 累加各次退避延迟，期望总等待落在约 7～9 分钟（与 Go 测试区间一致）。
#[test]
fn test_s3_tidb_retryer() {
    let retry = s3store::newRetryer();
    let error = anyhow!("timeout");
    let total_delay: Duration = (1..retry.MaxAttempts())
        .map(|attempt| retry.RetryDelay(attempt, &error).unwrap())
        .sum();
    assert!(total_delay > Duration::from_secs(7 * 60), "{total_delay:?}");
    assert!(total_delay < Duration::from_secs(9 * 60), "{total_delay:?}");
}

/// AWS Go SDK only applies full jitter before the configured 32-second cap.
#[test]
fn retry_delay_matches_aws_exponential_jitter_boundary() {
    let retry = s3store::newRetryer();
    let error = anyhow!("timeout");

    for attempt in 1..=5 {
        let upper_bound = Duration::from_secs(1_u64 << attempt);
        let delay = retry.RetryDelay(attempt, &error).unwrap();
        assert!(
            delay >= Duration::from_secs(1),
            "attempt {attempt}: {delay:?}"
        );
        assert!(delay < upper_bound, "attempt {attempt}: {delay:?}");
    }
    assert_eq!(
        retry.RetryDelay(6, &error).unwrap(),
        Duration::from_secs(32)
    );
}

/// 含 IMDS 地址的 deadline 错误不应按普通超时重试；普通 deadline 错误则可重试。
#[test]
fn test_retryer_is_instance_metadata_error() {
    let retry = s3store::newRetryer();
    assert!(!retry.IsErrorRetryable(&anyhow!("169.254.169.254 context deadline exceeded")));
    assert!(retry.IsErrorRetryable(&anyhow!("normal err: context deadline exceeded")));
}
