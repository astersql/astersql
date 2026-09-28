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

// S3 兼容存储相关 Prometheus 指标。
//
// 统计 BR 外部存储发起的 API 调用次数，以及读取路径上可重试错误次数，
// 便于观测后端类型（s3/oss/ks3）与具体 API（List/Head/Put）的调用分布。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::LazyLock;

/// 后端标签：标准 S3。
pub const BACKEND_S3: &str = "s3";
pub const BackendS3: &str = BACKEND_S3;
/// 后端标签：阿里云 OSS。
pub const BACKEND_OSS: &str = "oss";
pub const BackendOSS: &str = BACKEND_OSS;
/// 后端标签：金山云 KS3。
pub const BACKEND_KS3: &str = "ks3";
pub const BackendKS3: &str = BACKEND_KS3;
/// API 标签：列举对象。
pub const API_CALL_LIST_OBJECTS: &str = "ListObjects";
pub const APICallListObjects: &str = API_CALL_LIST_OBJECTS;
/// API 标签：Head 对象。
pub const API_CALL_HEAD_OBJECTS: &str = "HeadObjects";
pub const APICallHeadObjects: &str = API_CALL_HEAD_OBJECTS;
/// API 标签：上传对象。
pub const API_CALL_PUT_OBJECT: &str = "PutObject";
pub const APICallPutObject: &str = API_CALL_PUT_OBJECT;

/// S3 兼容 API 调用总次数（标签：backend、api）。
pub static S3_API_CALL_COUNTER: LazyLock<prometheus::CounterVec> = LazyLock::new(|| {
    let counter = metricscommon::NewCounterVec(
        prometheus::Opts::new(
            "api_call_total",
            "The total number of S3-compatible API calls made by BR external storage.",
        )
        .namespace("tidb")
        .subsystem("br_s3"),
        &["backend".to_owned(), "api".to_owned()],
    );
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("register S3 API call counter");
    counter
});
pub use S3_API_CALL_COUNTER as S3APICallCounter;

/// 可重试错误计数（标签：error），供 IO 读路径记录。
static RETRYABLE_ERROR_COUNTER: LazyLock<prometheus::CounterVec> = LazyLock::new(|| {
    prometheus::CounterVec::new(
        prometheus::Opts::new(
            "s3like_retryable_error_total",
            "Retryable errors observed by the S3-compatible storage implementation.",
        ),
        &["error"],
    )
    .expect("valid retryable error metric")
});

/// 将 `S3_API_CALL_COUNTER` 注册到默认 Prometheus registry（幂等）。
pub fn init() {
    LazyLock::force(&S3_API_CALL_COUNTER);
}

/// 按后端与 API 名称递增调用计数。
pub fn RecordAPICall(backend: &str, api: &str) {
    S3_API_CALL_COUNTER.with_label_values(&[backend, api]).inc();
}

/// 记录一次可重试错误文案对应的计数。
pub(crate) fn RecordRetryableError(error: &str) {
    RETRYABLE_ERROR_COUNTER.with_label_values(&[error]).inc();
}
