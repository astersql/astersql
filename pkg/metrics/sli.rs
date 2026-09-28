// Copyright 2021 PingCAP, Inc.
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

// SLI（Service Level Indicator，服务水平指标）相关 Prometheus 定义。
//
// 关注事务写入侧体验：小事务写耗时与一般事务写吞吐，用于衡量写入路径服务质量。
// 本文件只构造指标句柄，不注册采集器、不提交事务。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 仅构造 Prometheus 指标描述，不会注册采集器、连接数据库、提交事务或产生任何写入 IO。

// SmallTxnWriteDuration 收集小事务的写入耗时。
/// 小事务写入耗时直方图（秒）。
pub static mut SmallTxnWriteDuration: Option<prometheus::Histogram> = None;

// TxnWriteThroughput 收集非小事务的写入吞吐量。
/// 非小事务写入吞吐量直方图（字节/秒）。
pub static mut TxnWriteThroughput: Option<prometheus::Histogram> = None;

// InitSliMetrics 对应 Go 的 SLI 初始化函数；这里只替换包级句柄，不执行指标注册。
/// 初始化 SLI 写入体验相关指标句柄。
pub fn InitSliMetrics() {
    unsafe {
        SmallTxnWriteDuration = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "sli",
            Name: "small_txn_write_duration_seconds",
            Help: "Bucketed histogram of small transaction write time (s).",
            // 从 1ms 起按 2 倍增长，共 28 桶，覆盖到约 74 小时。
            Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 28),
            ..Default::default()
        }));

        TxnWriteThroughput = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "sli",
            Name: "txn_write_throughput",
            Help: "Bucketed histogram of transaction write throughput (bytes/second).",
            // 从 64 bytes/s 起按 1.3 倍增长，共 40 桶，覆盖到约 2.3MB/s。
            Buckets: prometheus::ExponentialBuckets(64.0, 1.3, 40),
            ..Default::default()
        }));
    }
}
