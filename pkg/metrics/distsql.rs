// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// DistSQL（分布式 SQL）与 Coprocessor 相关 Prometheus 指标。
//
// DistSQL 将查询下推到存储层（如 TiKV）执行；Coprocessor（协处理器）在 Region
// 上就地计算。本模块统计查询耗时、扫描 key 数、部分结果数、缓存命中以及响应体大小。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

//

// distsql metrics.
// Go 在 InitDistSQLMetrics 中写入包级变量；Option 保留“初始化前为空”的生命周期语义。
/// DistSQL 处理查询的耗时直方图（按 type/sql_type/copr_type）。
pub static mut DistSQLQueryHistogram: Option<prometheus::HistogramVec> = None;
/// 每个 partial 结果扫描的 key 数量直方图。
pub static mut DistSQLScanKeysPartialHistogram: Option<prometheus::Histogram> = None;
/// 每次查询扫描的 key 总量直方图。
pub static mut DistSQLScanKeysHistogram: Option<prometheus::Histogram> = None;
/// 每次查询产生的 partial 结果数量直方图。
pub static mut DistSQLPartialCountHistogram: Option<prometheus::Histogram> = None;
/// Coprocessor 缓存命中/淘汰/未命中计数。
pub static mut DistSQLCoprCacheCounter: Option<prometheus::CounterVec> = None;
/// Coprocessor 就近读（local read）命中计数。
pub static mut DistSQLCoprClosestReadCounter: Option<prometheus::CounterVec> = None;
/// Coprocessor 响应体大小直方图（按 store）。
pub static mut DistSQLCoprRespBodySize: Option<prometheus::HistogramVec> = None;

/// 初始化全部 DistSQL/Coprocessor 指标 collector。
// InitDistSQLMetrics 对应 Go 初始化函数：按原顺序构造查询、扫描、缓存与响应大小指标。
pub fn InitDistSQLMetrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    let query_histogram = metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "distsql",
            Name: "handle_query_duration_seconds",
            Help: "Bucketed histogram of processing time (s) of handled queries.",
            // 与 Go 的 0.0005、倍数 2、29 个桶一致，覆盖约 0.5ms 到 1.5 天。
            Buckets: prometheus::ExponentialBuckets(0.0005, 2.0, 29),
        },
        vec![LblType, LblSQLType, LblCoprType],
    );

    let scan_keys_partial = metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: "distsql",
        Name: "scan_keys_partial_num",
        Help: "number of scanned keys for each partial result.",
        ..Default::default()
    });

    let scan_keys = metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: "distsql",
        Name: "scan_keys_num",
        Help: "number of scanned keys for each query.",
        ..Default::default()
    });

    let partial_count = metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: "distsql",
        Name: "partial_num",
        Help: "number of partial results for each query.",
        ..Default::default()
    });

    let cache_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "distsql",
            Name: "copr_cache",
            Help: "coprocessor cache hit, evict and miss number",
        },
        vec![LblType],
    );

    let closest_read_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "distsql",
            Name: "copr_closest_read",
            Help: "counter of total copr read local read hit.",
        },
        vec![LblType],
    );

    let response_body_size = metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "distsql",
            Name: "copr_resp_size",
            Help: "copr task response data size in bytes.",
            Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 10),
        },
        vec![LblStore],
    );

    // 多个 static mut 赋值机械表达 Go 包级变量写入；真实并发初始化与注册机制留待模块接线阶段处理。
    unsafe {
        DistSQLQueryHistogram = Some(query_histogram);
        DistSQLScanKeysPartialHistogram = Some(scan_keys_partial);
        DistSQLScanKeysHistogram = Some(scan_keys);
        DistSQLPartialCountHistogram = Some(partial_count);
        DistSQLCoprCacheCounter = Some(cache_counter);
        DistSQLCoprClosestReadCounter = Some(closest_read_counter);
        DistSQLCoprRespBodySize = Some(response_body_size);
    }
}
