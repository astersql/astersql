// Copyright 2023 PingCAP, Inc.
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

// 规划器 core 的 Prometheus 指标定义。
//
// 覆盖：伪统计估计（pseudo estimation，因过期或缺失统计信息触发）、
// 计划缓存（plan cache）命中/未命中、实例级计划数与内存占用、
// 以及查找/克隆耗时直方图。对应 Go `planner/core/metrics`。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::LazyLock;

use prometheus::{
    Counter, CounterVec, Gauge, GaugeVec, Histogram, HistogramOpts, HistogramVec, Opts,
};

/// 构造带 `type` 标签的 CounterVec，命名空间固定为 `tidb`。
fn counter_vec(subsystem: &str, name: &str, help: &str) -> CounterVec {
    CounterVec::new(
        Opts::new(name, help).namespace("tidb").subsystem(subsystem),
        &["type"],
    )
    .expect("planner metric descriptor must be valid")
}

/// 构造 server 子系统下的 GaugeVec。
fn gauge_vec(name: &str, help: &str) -> GaugeVec {
    GaugeVec::new(
        Opts::new(name, help).namespace("tidb").subsystem("server"),
        &["type"],
    )
    .expect("planner metric descriptor must be valid")
}

// 伪估计计数：统计信息缺失或过期时优化器回退到默认估计。
static PseudoEstimation: LazyLock<CounterVec> = LazyLock::new(|| {
    counter_vec(
        "statistics",
        "pseudo_estimation_total",
        "Counter of pseudo estimation caused by outdated stats.",
    )
});

// 计划缓存命中计数（按 prepared / non-prepared 区分）。
static PlanCacheCounter: LazyLock<CounterVec> = LazyLock::new(|| {
    counter_vec(
        "server",
        "plan_cache_total",
        "Counter of query using plan cache.",
    )
});

// 计划缓存未命中计数。
static PlanCacheMissCounter: LazyLock<CounterVec> = LazyLock::new(|| {
    counter_vec(
        "server",
        "plan_cache_miss_total",
        "Counter of plan cache miss.",
    )
});

// 实例内已缓存计划条数。
static PlanCacheInstancePlanNumCounter: LazyLock<GaugeVec> = LazyLock::new(|| {
    gauge_vec(
        "plan_cache_instance_plan_num_total",
        "Counter of plan of all prepared plan cache in a instance",
    )
});

// 实例内计划缓存占用的总内存。
static PlanCacheInstanceMemoryUsage: LazyLock<GaugeVec> = LazyLock::new(|| {
    gauge_vec(
        "plan_cache_instance_memory_usage",
        "Total plan cache memory usage of all sessions in a instance",
    )
});

// 计划缓存查找/克隆等操作的耗时分布。
static PlanCacheProcessDuration: LazyLock<HistogramVec> = LazyLock::new(|| {
    HistogramVec::new(
        HistogramOpts::new(
            "plan_cache_process_duration_seconds",
            "Bucketed histogram of processing time (s) of plan cache operations.",
        )
        .namespace("tidb")
        .subsystem("server")
        .buckets(prometheus::exponential_buckets(0.001, 2.0, 28).expect("valid buckets")),
        &["type"],
    )
    .expect("planner metric descriptor must be valid")
});

/// 无可用统计信息时的伪估计计数器。
pub static PseudoEstimationNotAvailable: LazyLock<Counter> =
    LazyLock::new(|| PseudoEstimation.with_label_values(&["nodata"]));
/// 统计信息过期时的伪估计计数器。
pub static PseudoEstimationOutdate: LazyLock<Counter> =
    LazyLock::new(|| PseudoEstimation.with_label_values(&["outdate"]));

// 以下为按标签预绑定的具体句柄，避免热路径反复 with_label_values。
static preparedPlanCacheHitCounter: LazyLock<Counter> =
    LazyLock::new(|| PlanCacheCounter.with_label_values(&["prepared"]));
static nonPreparedPlanCacheHitCounter: LazyLock<Counter> =
    LazyLock::new(|| PlanCacheCounter.with_label_values(&["non-prepared"]));
static preparedPlanCacheMissCounter: LazyLock<Counter> =
    LazyLock::new(|| PlanCacheMissCounter.with_label_values(&["prepared"]));
static nonPreparedPlanCacheMissCounter: LazyLock<Counter> =
    LazyLock::new(|| PlanCacheMissCounter.with_label_values(&["non-prepared"]));
static nonPreparedPlanCacheUnsupportedCounter: LazyLock<Counter> =
    LazyLock::new(|| PlanCacheMissCounter.with_label_values(&["non-prepared-unsupported"]));
static sessionPlanCacheInstancePlanNumCounter: LazyLock<Gauge> =
    LazyLock::new(|| PlanCacheInstancePlanNumCounter.with_label_values(&[" session-plan-cache"]));
static sessionPlanCacheInstanceMemoryUsage: LazyLock<Gauge> =
    LazyLock::new(|| PlanCacheInstanceMemoryUsage.with_label_values(&[" session-plan-cache"]));
static instancePlanCacheInstancePlanNumCounter: LazyLock<Gauge> =
    LazyLock::new(|| PlanCacheInstancePlanNumCounter.with_label_values(&[" instance-plan-cache"]));
static instancePlanCacheInstanceMemoryUsage: LazyLock<Gauge> =
    LazyLock::new(|| PlanCacheInstanceMemoryUsage.with_label_values(&[" instance-plan-cache"]));
static instancePlanCacheInstanceNumEvict: LazyLock<Gauge> = LazyLock::new(|| {
    PlanCacheInstancePlanNumCounter.with_label_values(&[" instance-plan-cache-last-evict"])
});
static sessionPlanCacheLookupDuration: LazyLock<Histogram> =
    LazyLock::new(|| PlanCacheProcessDuration.with_label_values(&[" session-plan-cache-lookup"]));
static instancePlanCacheLookupDuration: LazyLock<Histogram> =
    LazyLock::new(|| PlanCacheProcessDuration.with_label_values(&[" instance-plan-cache-lookup"]));
static instancePlanCacheCloneDuration: LazyLock<Histogram> =
    LazyLock::new(|| PlanCacheProcessDuration.with_label_values(&[" instance-plan-cache-clone"]));

/// 初始化包级指标句柄（惰性强制求值）。
pub fn init() {
    InitMetricsVars();
}

/// 按 Go 绑定顺序强制初始化全部包级指标句柄。
/// Initializes every package-level metric handle, preserving the Go binding order.
pub fn InitMetricsVars() {
    LazyLock::force(&PseudoEstimationNotAvailable);
    LazyLock::force(&PseudoEstimationOutdate);
    LazyLock::force(&preparedPlanCacheHitCounter);
    LazyLock::force(&nonPreparedPlanCacheHitCounter);
    LazyLock::force(&preparedPlanCacheMissCounter);
    LazyLock::force(&nonPreparedPlanCacheMissCounter);
    LazyLock::force(&nonPreparedPlanCacheUnsupportedCounter);
    LazyLock::force(&sessionPlanCacheInstancePlanNumCounter);
    LazyLock::force(&sessionPlanCacheInstanceMemoryUsage);
    LazyLock::force(&instancePlanCacheInstancePlanNumCounter);
    LazyLock::force(&instancePlanCacheInstanceMemoryUsage);
    LazyLock::force(&instancePlanCacheInstanceNumEvict);
    LazyLock::force(&sessionPlanCacheLookupDuration);
    LazyLock::force(&instancePlanCacheLookupDuration);
    LazyLock::force(&instancePlanCacheCloneDuration);
}

/// 返回计划缓存命中计数器；`isNonPrepared` 区分非预处理与预处理语句。
pub fn GetPlanCacheHitCounter(isNonPrepared: bool) -> Counter {
    if isNonPrepared {
        nonPreparedPlanCacheHitCounter.clone()
    } else {
        preparedPlanCacheHitCounter.clone()
    }
}

/// 返回计划缓存未命中计数器。
pub fn GetPlanCacheMissCounter(isNonPrepared: bool) -> Counter {
    if isNonPrepared {
        nonPreparedPlanCacheMissCounter.clone()
    } else {
        preparedPlanCacheMissCounter.clone()
    }
}

/// 非预处理计划缓存因不支持而跳过的计数器。
pub fn GetNonPrepPlanCacheUnsupportedCounter() -> Counter {
    nonPreparedPlanCacheUnsupportedCounter.clone()
}

/// 返回实例或会话级计划缓存条数仪表。
pub fn GetPlanCacheInstanceNumCounter(instancePlanCache: bool) -> Gauge {
    if instancePlanCache {
        instancePlanCacheInstancePlanNumCounter.clone()
    } else {
        sessionPlanCacheInstancePlanNumCounter.clone()
    }
}

/// 返回实例或会话级计划缓存内存占用仪表。
pub fn GetPlanCacheInstanceMemoryUsage(instancePlanCache: bool) -> Gauge {
    if instancePlanCache {
        instancePlanCacheInstanceMemoryUsage.clone()
    } else {
        sessionPlanCacheInstanceMemoryUsage.clone()
    }
}

/// 返回实例计划缓存克隆耗时直方图。
pub fn GetPlanCacheCloneDuration() -> Histogram {
    instancePlanCacheCloneDuration.clone()
}

/// 返回计划缓存查找耗时直方图。
pub fn GetPlanCacheLookupDuration(instancePlanCache: bool) -> Histogram {
    if instancePlanCache {
        instancePlanCacheLookupDuration.clone()
    } else {
        sessionPlanCacheLookupDuration.clone()
    }
}

/// 返回最近一次实例计划缓存淘汰条数仪表。
pub fn GetPlanCacheInstanceEvict() -> Gauge {
    instancePlanCacheInstanceNumEvict.clone()
}
