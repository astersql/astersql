// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// InfoSchema V2 缓存与按名查表相关 Prometheus 指标。
//
// InfoSchema（信息模式）描述库表等元数据；V2 缓存以内存换取查表延迟。本模块统计
// 缓存命中/未命中/淘汰、对象数、内存占用与上限，以及 `TableByName` API 的 hit/miss 耗时。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// Option 表达 Go 包变量在 InitInfoSchemaV2Metrics 调用前尚未绑定 collector 的状态。
/// InfoSchema V2 缓存命中/淘汰/未命中计数。
// InfoSchemaV2CacheCounter 记录 InfoSchema V2 缓存的命中、未命中与淘汰次数。
pub static mut InfoSchemaV2CacheCounter: Option<prometheus::CounterVec> = None;
/// 缓存占用内存字节数。
// InfoSchemaV2CacheMemUsage 记录缓存占用的内存字节数。
pub static mut InfoSchemaV2CacheMemUsage: Option<prometheus::Gauge> = None;
/// 缓存中的表对象数。
// InfoSchemaV2CacheObjCnt 记录缓存中的表对象数。
pub static mut InfoSchemaV2CacheObjCnt: Option<prometheus::Gauge> = None;
/// 缓存内存上限。
// InfoSchemaV2CacheMemLimit 记录缓存的内存上限。
pub static mut InfoSchemaV2CacheMemLimit: Option<prometheus::Gauge> = None;
/// TableByName API 耗时直方图。
// TableByNameDuration 记录 InfoSchema V2 TableByName API 的耗时分布。
pub static mut TableByNameDuration: Option<prometheus::HistogramVec> = None;
/// 预绑定 type=hit 的 TableByName 耗时 Observer。
// TableByNameHitDuration 是预绑定 type="hit" 标签的 observer。
pub static mut TableByNameHitDuration: Option<prometheus::Observer> = None;
/// 预绑定 type=miss 的 TableByName 耗时 Observer。
// TableByNameMissDuration 是预绑定 type="miss" 标签的 observer。
pub static mut TableByNameMissDuration: Option<prometheus::Observer> = None;

/// 初始化 InfoSchema V2 缓存与 TableByName 耗时指标，并派生 hit/miss Observer。
// InitInfoSchemaV2Metrics 对应 Go 的同名函数，构造缓存状态和查询耗时 collector。
// 可变全局量仅用于保留 Go 初始化形状；后续可编译迁移应改用线程安全的一次性初始化容器。
pub unsafe fn InitInfoSchemaV2Metrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    InfoSchemaV2CacheCounter =
        Some((*astersql_infoschema_metrics::InfoSchemaV2CacheCounter).clone());
    InfoSchemaV2CacheObjCnt = Some((*astersql_infoschema_metrics::InfoSchemaV2CacheObjCnt).clone());
    InfoSchemaV2CacheMemUsage =
        Some((*astersql_infoschema_metrics::InfoSchemaV2CacheMemUsage).clone());
    InfoSchemaV2CacheMemLimit =
        Some((*astersql_infoschema_metrics::InfoSchemaV2CacheMemLimit).clone());

    TableByNameDuration = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "infoschema",
            Name: "table_by_name_duration_nanoseconds",
            Help: "infoschema v2 TableByName API duration",
            // 保留 Go 的指数桶：从 1 开始按 2 倍增长，共 30 个桶。
            Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 30),
            ..Default::default()
        },
        vec![LblType],
    ));

    // Go 从同一个 HistogramVec 派生固定标签 observer，确保 hit/miss 仍汇入相同指标族。
    let duration = TableByNameDuration
        .as_ref()
        .expect("TableByNameDuration must be initialized first");
    TableByNameHitDuration = Some(duration.WithLabelValues(vec!["hit"]));
    TableByNameMissDuration = Some(duration.WithLabelValues(vec!["miss"]));
}
