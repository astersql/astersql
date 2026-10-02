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

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::anyhow;
use aws_sdk_s3::error::ErrorMetadata;
use aws_sdk_s3::operation::head_bucket::HeadBucketError;
use storeapi::aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use storeapi::aws_smithy_runtime_api::client::result::SdkError;

fn region_error(code: &str, status: u16) -> SdkError<HeadBucketError, HttpResponse> {
    let response = HttpResponse::new(
        status.try_into().unwrap(),
        aws_sdk_s3::primitives::SdkBody::empty(),
    );
    SdkError::service_error(
        HeadBucketError::generic(ErrorMetadata::builder().code(code).build()),
        response,
    )
}

#[derive(Clone)]
struct RecordedWarnings(Arc<Mutex<Vec<u8>>>);

impl Write for RecordedWarnings {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn go_commit_c50aae2b1b_redirect_classifier_requires_code_and_status() {
    for code in ["MovedPermanently", "PermanentRedirect"] {
        assert!(s3store::isBucketRegionRedirectError(&region_error(
            code, 301
        )));
    }
    assert!(!s3store::isBucketRegionRedirectError(&region_error(
        "MovedPermanently",
        403
    )));
    assert!(!s3store::isBucketRegionRedirectError(&region_error(
        "AccessDenied",
        301
    )));
    let no_response: SdkError<HeadBucketError, HttpResponse> =
        SdkError::construction_failure("missing response");
    assert!(!s3store::isBucketRegionRedirectError(&no_response));
    let code_from_empty_301 = SdkError::service_error(
        HeadBucketError::generic(ErrorMetadata::builder().build()),
        HttpResponse::new(
            301.try_into().unwrap(),
            aws_sdk_s3::primitives::SdkBody::empty(),
        ),
    );
    assert!(s3store::isBucketRegionRedirectError(&code_from_empty_301));
}

#[test]
fn go_commit_c50aae2b1b_only_probe_suppresses_expected_warning() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer({
            let output = output.clone();
            move || RecordedWarnings(output.clone())
        })
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let redirect = anyhow::Error::new(region_error("MovedPermanently", 301));
        let _ = s3store::newBucketRegionDetectionRetryer().IsErrorRetryable(&redirect);
        assert!(output.lock().unwrap().is_empty());

        let _ = s3store::newRetryer().IsErrorRetryable(&redirect);
        let normal_warning = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert_eq!(normal_warning.matches("failed to request s3").count(), 1);

        let unexpected = anyhow::Error::new(region_error("AccessDenied", 301));
        let _ = s3store::newBucketRegionDetectionRetryer().IsErrorRetryable(&unexpected);
        let all_warnings = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert_eq!(all_warnings.matches("failed to request s3").count(), 2);
    });
}

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
