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

// Ingest 路径 Prometheus 耗时指标。
//
// 用带 `api` 标签的 HistogramVec 同时覆盖 write（写 SST）与 ingest（灌入 TiKV）
// 两类 API；初始化后通过 `Register` 挂到全局 Registry。

use std::sync::{LazyLock, RwLock};

/// HistogramVec 上区分 write/ingest 的标签名。
const lblAPI: &str = "api";
/// write API 的标签取值。
pub const LabelWriteAPI: &str = "write";
/// ingest API 的标签取值。
pub const LabelIngestAPI: &str = "ingest";

/// Records the duration of write and ingest APIs in nextgen.
/// 带标签的 write/ingest API 耗时直方图（HistogramVec）。
pub static WriteIngestAPIDuration: LazyLock<RwLock<Option<prometheus::HistogramVec>>> =
    LazyLock::new(|| RwLock::new(None));
/// Records the duration of the write API.
/// write API 专用观察器，绑定 `LabelWriteAPI`。
pub static WriteAPIDuration: LazyLock<RwLock<Option<prometheus::Histogram>>> =
    LazyLock::new(|| RwLock::new(None));
/// Records the duration of the ingest API.
/// ingest API 专用观察器，绑定 `LabelIngestAPI`。
pub static IngestAPIDuration: LazyLock<RwLock<Option<prometheus::Histogram>>> =
    LazyLock::new(|| RwLock::new(None));

/// Initializes ingest metrics.
/// 创建指数桶直方图并为 write/ingest 各取一个带标签观察器。
pub fn InitIngestMetrics() {
    // 从 1ms 起指数增长 20 档，覆盖短请求到分钟级导入
    let buckets =
        prometheus::exponential_buckets(0.001, 2.0, 20).expect("valid ingest duration buckets");
    let histogram = metricscommon::NewHistogramVec(
        prometheus::HistogramOpts::new(
            "write_ingest_api_duration",
            "write and ingest API duration",
        )
        .namespace("tidb")
        .subsystem("ingestor")
        .buckets(buckets),
        &[lblAPI.to_owned()],
    );
    let write_observer = histogram.with_label_values(&[LabelWriteAPI]);
    let ingest_observer = histogram.with_label_values(&[LabelIngestAPI]);

    *WriteIngestAPIDuration.write().unwrap() = Some(histogram);
    *WriteAPIDuration.write().unwrap() = Some(write_observer);
    *IngestAPIDuration.write().unwrap() = Some(ingest_observer);
}

/// Registers ingest metrics, panicking on missing initialization or registration errors.
/// 将已初始化的 HistogramVec 注册到 Prometheus Registry；未先 Init 会 panic。
pub fn Register(register: &prometheus::Registry) {
    let histogram = WriteIngestAPIDuration
        .read()
        .unwrap()
        .clone()
        .expect("InitIngestMetrics must be called before Register");
    register
        .register(Box::new(histogram))
        .expect("register ingest metrics");
}
