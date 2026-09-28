// Copyright 2026 AsterSQL.
// Copyright 2026 TiKV Authors.
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

//! TiKV client-go metrics referenced by `metrics.ToggleSimplifiedMode`.
//!
//! AsterSQL uses the Rust TiKV client, whose private Prometheus 0.13 registry
//! cannot provide the client-go 0.14 collectors consumed by TiDB dashboards.
//! These compatibility collectors preserve the client-go descriptors and are
//! shared with the simplified-mode registration lifecycle.
//!
//! TiKV 客户端兼容指标。
//!
//! AsterSQL 使用 Rust TiKV 客户端，但其私有的 Prometheus 0.13 注册表无法提供
//! TiDB 仪表盘依赖的 client-go 0.14 collector。本模块保留 client-go 的指标描述，
//! 并让这些兼容 collector 参与简化指标模式的注册与注销流程。

use prometheus::core::Collector;
use prometheus::{Counter, Histogram, HistogramVec, Registry};
use std::sync::LazyLock;

const NAMESPACE: &str = "tidb";
const SUBSYSTEM: &str = "tikvclient";

/// 按数据类型统计 RawKV 写入键和值的字节大小。
pub static TiKVRawkvSizeHistogram: LazyLock<HistogramVec> = LazyLock::new(|| {
    astersql_metrics_common::NewHistogramVec(
        prometheus::HistogramOpts::new(
            "rawkv_kv_size_bytes",
            "Size of key/value to put, in bytes.",
        )
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM)
        .buckets(prometheus::exponential_buckets(1.0, 2.0, 30).unwrap()),
        &["type".to_owned()],
    )
});

/// 按命令类型统计 RawKV 命令的处理耗时。
pub static TiKVRawkvCmdHistogram: LazyLock<HistogramVec> = LazyLock::new(|| {
    astersql_metrics_common::NewHistogramVec(
        prometheus::HistogramOpts::new(
            "rawkv_cmd_seconds",
            "Bucketed histogram of processing time of rawkv cmds.",
        )
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM)
        .buckets(prometheus::exponential_buckets(0.0005, 2.0, 29).unwrap()),
        &["type".to_owned()],
    )
});

/// TiKV 读取吞吐量，归入 SLI 子系统。
pub static TiKVReadThroughput: LazyLock<Histogram> = LazyLock::new(|| {
    astersql_metrics_common::NewHistogram(
        prometheus::HistogramOpts::new(
            "tikv_read_throughput",
            "Read throughput of TiKV read in Bytes/s.",
        )
        .namespace(NAMESPACE)
        .subsystem("sli")
        .buckets(prometheus::exponential_buckets(1024.0, 2.0, 13).unwrap()),
    )
});

/// TiKV 小读取请求的耗时，归入 SLI 子系统。
pub static TiKVSmallReadDuration: LazyLock<Histogram> = LazyLock::new(|| {
    astersql_metrics_common::NewHistogram(
        prometheus::HistogramOpts::new("tikv_small_read_duration", "Read time of TiKV small read.")
            .namespace(NAMESPACE)
            .subsystem("sli")
            .buckets(prometheus::exponential_buckets(0.0005, 2.0, 28).unwrap()),
    )
});

/// TiKV 传输层批处理等待因过载触发的事件总数。
pub static TiKVBatchWaitOverLoad: LazyLock<Counter> = LazyLock::new(|| {
    astersql_metrics_common::NewCounter(
        prometheus::Opts::new(
            "batch_wait_overload",
            "event of tikv transport layer overload",
        )
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM),
    )
});

/// 批处理客户端回收连接并重新连接的耗时。
pub static TiKVBatchClientRecycle: LazyLock<Histogram> = LazyLock::new(|| {
    astersql_metrics_common::NewHistogram(
        prometheus::HistogramOpts::new(
            "batch_client_reset",
            "batch client recycle connection and reconnect duration",
        )
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM)
        .buckets(prometheus::exponential_buckets(0.001, 2.0, 28).unwrap()),
    )
});

/// 单次 Region 请求的重试次数分布。
pub static TiKVRequestRetryTimesHistogram: LazyLock<Histogram> = LazyLock::new(|| {
    astersql_metrics_common::NewHistogram(
        prometheus::HistogramOpts::new(
            "request_retry_times",
            "Bucketed histogram of how many times a region request retries.",
        )
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM)
        .buckets(vec![
            1.0, 2.0, 3.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0,
        ]),
    )
});

/// 按 TiKV store 统计状态 API 的调用耗时。
pub static TiKVStatusDuration: LazyLock<HistogramVec> = LazyLock::new(|| {
    astersql_metrics_common::NewHistogramVec(
        prometheus::HistogramOpts::new("kv_status_api_duration", "duration for kv status api.")
            .namespace(NAMESPACE)
            .subsystem(SUBSYSTEM)
            .buckets(prometheus::exponential_buckets(0.0005, 2.0, 20).unwrap()),
        &["store".to_owned()],
    )
});

/// 强制初始化全部延迟创建的兼容指标，确保后续注册取得完整集合。
pub fn InitMetrics() {
    let _ = &*TiKVRawkvSizeHistogram;
    let _ = &*TiKVRawkvCmdHistogram;
    let _ = &*TiKVReadThroughput;
    let _ = &*TiKVSmallReadDuration;
    let _ = &*TiKVBatchWaitOverLoad;
    let _ = &*TiKVBatchClientRecycle;
    let _ = &*TiKVRequestRetryTimesHistogram;
    let _ = &*TiKVStatusDuration;
}

/// 返回兼容指标的 collector 克隆，供常规注册及简化模式切换复用。
///
/// 克隆只复制 Prometheus 句柄，底层指标状态仍由各全局静态项共享。
pub fn unused_collectors() -> Vec<Box<dyn Collector>> {
    InitMetrics();
    vec![
        Box::new((*TiKVRawkvSizeHistogram).clone()),
        Box::new((*TiKVRawkvCmdHistogram).clone()),
        Box::new((*TiKVReadThroughput).clone()),
        Box::new((*TiKVSmallReadDuration).clone()),
        Box::new((*TiKVBatchWaitOverLoad).clone()),
        Box::new((*TiKVBatchClientRecycle).clone()),
        Box::new((*TiKVRequestRetryTimesHistogram).clone()),
        Box::new((*TiKVStatusDuration).clone()),
    ]
}

/// 将全部 TiKV 客户端兼容指标注册到指定注册表。
pub fn RegisterMetrics(registry: &Registry) -> prometheus::Result<()> {
    for collector in unused_collectors() {
        registry.register(collector)?;
    }
    Ok(())
}
