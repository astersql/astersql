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

// Meta / AutoID 相关 Prometheus 指标定义与初始化。
//
// 覆盖自动分配 ID（AutoID）操作耗时、schema diff / 历史 DDL 作业等元数据操作延迟，
// 以及重置 AutoID 客户端连接的次数。DDL 指数据定义语言；schema diff 记录元信息变更增量。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

//

// Metrics
// 这些字符串沿用 Go 指标标签值，Option 表示 Go 中初始化前可为 nil 的全局指标指针。
/// 全局 AutoID 操作类型标签值。
pub static GLOBAL_AUTO_ID: &str = "global";
/// 表级 AutoID 分配（alloc）操作标签值。
pub static TABLE_AUTO_ID_ALLOC: &str = "alloc";
/// 表级 AutoID rebase 操作标签值。
pub static TABLE_AUTO_ID_REBASE: &str = "rebase";
/// AutoID 处理耗时直方图（按类型与结果标签）。
pub static mut AUTO_ID_HISTOGRAM: Option<prometheus::HistogramVec> = None;

/// 读取 schema diff 的操作类型标签。
pub static GET_SCHEMA_DIFF: &str = "get_schema_diff";
/// 写入 schema diff 的操作类型标签。
pub static SET_SCHEMA_DIFF: &str = "set_schema_diff";
/// 读取历史 DDL 作业的操作类型标签。
pub static GET_HISTORY_DDL_JOB: &str = "get_history_ddl_job";

/// 元数据操作耗时直方图（按类型与结果标签）。
pub static mut META_HISTOGRAM: Option<prometheus::HistogramVec> = None;
/// 重置 AutoID 客户端连接次数计数器。
pub static mut RESET_AUTO_ID_CONN_COUNTER: Option<prometheus::Counter> = None;

// init_meta_metrics 对应 Go 的 InitMetaMetrics：构造 AutoID、meta 延迟直方图与连接重置计数器。
// 对 static mut 的赋值仅保留 Go 包级变量的形状；后续接线时应换成 OnceLock 等安全单例。
/// 初始化本文件全部 meta / AutoID 指标。
pub unsafe fn init_meta_metrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    AUTO_ID_HISTOGRAM = Some(metricscommon::new_histogram_vec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "autoid",
            Name: "operation_duration_seconds",
            Help: "Bucketed histogram of processing time (s) of handled autoid.",
            // 与 Go 的 0.0005 * 2^n 桶一致，覆盖约 0.5ms 到 1.5 天。
            Buckets: prometheus::exponential_buckets(0.0005, 2.0, 29),
        },
        &[LBL_TYPE, LBL_RESULT],
    ));

    META_HISTOGRAM = Some(metricscommon::new_histogram_vec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "meta",
            Name: "operation_duration_seconds",
            Help: "Bucketed histogram of processing time (s) of tidb meta data operations.",
            // Go 使用相同的指数桶，以便两类操作延迟可以采用一致的观察尺度。
            Buckets: prometheus::exponential_buckets(0.0005, 2.0, 29),
        },
        &[LBL_TYPE, LBL_RESULT],
    ));

    RESET_AUTO_ID_CONN_COUNTER = Some(metricscommon::new_counter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: "meta",
        Name: "autoid_client_conn_reset_total",
        Help: "Counter of resetting autoid client connection.",
    }));
}
