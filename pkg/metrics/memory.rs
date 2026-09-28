// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 全局内存仲裁器（memory arbitrator）相关 Prometheus 指标。
//
// 跟踪 SQL 执行过程中的配额仲裁耗时、工作模式、等待任务数、根内存池状态，
// 以及按标签细分的事件与任务执行计数。内存仲裁用于在多会话间分配与限流内存配额。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

// namespace 与 subsystem 对应 Go 的包内常量，供本文件所有 collector 复用。
/// Prometheus 命名空间，固定为 tidb。
const namespace: &str = "tidb";
/// Prometheus 子系统名，固定为 memory。
const subsystem: &str = "memory";

// Memory metrics。Option 表达 Go 包变量在 InitMemoryMetrics 调用前仍是零值。
/// SQL 执行中内存配额仲裁耗时直方图（秒）。
pub static mut GlobalMemArbitrationDuration: Option<prometheus::Histogram> = None;
/// 全局内存仲裁器当前工作模式。
pub static mut GlobalMemArbitratorWorkMode: Option<prometheus::GaugeVec> = None;
/// 全局内存仲裁器配额信息（字节）。
pub static mut GlobalMemArbitratorQuota: Option<prometheus::GaugeVec> = None;
/// 等待仲裁的任务数量。
pub static mut GlobalMemArbitratorWaitingTask: Option<prometheus::GaugeVec> = None;
/// 运行时 heapinuse 相对配额的放大比率。
pub static mut GlobalMemArbitratorRuntimeMemMagnifi: Option<prometheus::Gauge> = None;
/// 根内存池（root pool）状态。
pub static mut GlobalMemArbitratorRootPool: Option<prometheus::GaugeVec> = None;
/// 仲裁器事件计数（按类型标签）。
pub static mut GlobalMemArbitratorEventCounter: Option<prometheus::CounterVec> = None;
/// 仲裁器任务执行计数（按类型标签）。
pub static mut GlobalMemArbitratorTaskExecCounter: Option<prometheus::CounterVec> = None;

// GlobalMemArbitratorSubEvents 对应 Go 的匿名结构体，为常用 event 标签预先保存 Counter。
/// 常用 pool-init 相关事件的预绑定 Counter 集合。
#[derive(Default)]
pub struct GlobalMemArbitratorSubEventsMetrics {
    pub PoolInitHitDigest: Option<prometheus::Counter>,
    pub PoolInitReserve: Option<prometheus::Counter>,
    pub PoolInitMediumQuota: Option<prometheus::Counter>,
    pub PoolInitNone: Option<prometheus::Counter>,
}

/// 全局内存仲裁器常用事件子指标的包级实例。
pub static mut GlobalMemArbitratorSubEvents: GlobalMemArbitratorSubEventsMetrics =
    GlobalMemArbitratorSubEventsMetrics {
        PoolInitHitDigest: None,
        PoolInitReserve: None,
        PoolInitMediumQuota: None,
        PoolInitNone: None,
    };

// GlobalMemArbitratorSubTasks 对应 Go 的匿名结构体，为解析、计划和强制终止路径预绑定标签。
/// 解析 / 计划路径上取消、强制终止与无限制模式的预绑定 Counter 集合。
#[derive(Default)]
pub struct GlobalMemArbitratorSubTasksMetrics {
    pub CancelWaitAverseParse: Option<prometheus::Counter>,
    pub CancelWaitAversePlan: Option<prometheus::Counter>,
    pub CancelStandardModeParse: Option<prometheus::Counter>,
    pub CancelStandardModePlan: Option<prometheus::Counter>,
    pub ForceKillParse: Option<prometheus::Counter>,
    pub ForceKillPlan: Option<prometheus::Counter>,
    pub NoLimit: Option<prometheus::Counter>,
}

/// 全局内存仲裁器常用任务子指标的包级实例。
pub static mut GlobalMemArbitratorSubTasks: GlobalMemArbitratorSubTasksMetrics =
    GlobalMemArbitratorSubTasksMetrics {
        CancelWaitAverseParse: None,
        CancelWaitAversePlan: None,
        CancelStandardModeParse: None,
        CancelStandardModePlan: None,
        ForceKillParse: None,
        ForceKillPlan: None,
        NoLimit: None,
    };

// Go 的 counters/gauges 匿名结构体把 map 与 sync.RWMutex 放在一起。
// Rust 用 RwLock<HashMap> 保留多读单写语义，并以 LazyLock 代替 nil map 的首次分配。
/// 按 taskType 缓存的 Counter 句柄，避免重复 WithLabelValues。
static counters: LazyLock<RwLock<HashMap<String, prometheus::Counter>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
/// 按 taskType 缓存的 Gauge 句柄。
static gauges: LazyLock<RwLock<HashMap<String, prometheus::Gauge>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

// InitMemoryMetrics 对应 Go 的同名函数，初始化全局内存仲裁 collector 和常用标签子指标。
// 可变全局赋值只保留 Go 包级初始化顺序；真正接线时应由线程安全的一次初始化容器统一发布。
/// 初始化本文件全部内存仲裁指标，并派生常用标签子 Counter。
pub unsafe fn InitMemoryMetrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    GlobalMemArbitrationDuration = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: namespace,
        Subsystem: subsystem,
        Name: "arbitration_duration_seconds",
        Help: "Bucketed histogram of mem quota arbitration time (s) in SQL execution",
        // 与 Go 一致，从 50 微秒覆盖到 1 天并生成 17 个指数桶。
        Buckets: prometheus::ExponentialBucketsRange(0.00005, 3600.0 * 24.0, 17),
        ..Default::default()
    }));

    GlobalMemArbitratorQuota = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: namespace,
            Subsystem: subsystem,
            Name: "arbitrator_quota_bytes",
            Help: "Quota info of the global memory arbitrator",
            ..Default::default()
        },
        vec![LblType],
    ));

    GlobalMemArbitratorWorkMode = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: namespace,
            Subsystem: subsystem,
            Name: "arbitrator_work_mode",
            Help: "Work mode of the global memory arbitrator",
            ..Default::default()
        },
        vec![LblType],
    ));

    GlobalMemArbitratorWaitingTask = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: namespace,
            Subsystem: subsystem,
            Name: "arbitrator_waiting_task",
            Help: "Waiting task num of the global memory arbitrator",
            ..Default::default()
        },
        vec![LblType],
    ));

    GlobalMemArbitratorRuntimeMemMagnifi = Some(metricscommon::NewGauge(prometheus::GaugeOpts {
        Namespace: namespace,
        Subsystem: subsystem,
        Name: "arbitrator_magnifi_ratio",
        Help: "Runtime profile (heapinuse vs. quota) of the global memory arbitrator",
        ..Default::default()
    }));

    GlobalMemArbitratorRootPool = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: namespace,
            Subsystem: subsystem,
            Name: "arbitrator_root_pool",
            Help: "Root pool info of the global memory arbitrator",
            ..Default::default()
        },
        vec![LblType],
    ));

    GlobalMemArbitratorTaskExecCounter = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: namespace,
            Subsystem: subsystem,
            Name: "arbitrator_task_exec",
            Help: "Task execution count of the global memory arbitrator",
            ..Default::default()
        },
        vec![LblType],
    ));

    GlobalMemArbitratorEventCounter = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: namespace,
            Subsystem: subsystem,
            Name: "arbitrator_event",
            Help: "Event count of the global memory arbitrator",
            ..Default::default()
        },
        vec![LblType],
    ));

    // 先初始化父 CounterVec，再派生固定标签 Counter；顺序与 Go 一致，避免从空父指标取子项。
    let event_counter = GlobalMemArbitratorEventCounter
        .as_ref()
        .expect("GlobalMemArbitratorEventCounter must be initialized first");
    GlobalMemArbitratorSubEvents.PoolInitHitDigest =
        Some(event_counter.WithLabelValues(vec!["pool-init-hit-digest"]));
    GlobalMemArbitratorSubEvents.PoolInitReserve =
        Some(event_counter.WithLabelValues(vec!["pool-init-reserve"]));
    GlobalMemArbitratorSubEvents.PoolInitMediumQuota =
        Some(event_counter.WithLabelValues(vec!["pool-init-medium-quota"]));
    GlobalMemArbitratorSubEvents.PoolInitNone =
        Some(event_counter.WithLabelValues(vec!["pool-init-none"]));

    let task_counter = GlobalMemArbitratorTaskExecCounter
        .as_ref()
        .expect("GlobalMemArbitratorTaskExecCounter must be initialized first");
    GlobalMemArbitratorSubTasks.CancelWaitAverseParse =
        Some(task_counter.WithLabelValues(vec!["cancel-wait-averse-parse"]));
    GlobalMemArbitratorSubTasks.CancelWaitAversePlan =
        Some(task_counter.WithLabelValues(vec!["cancel-wait-averse-plan"]));
    GlobalMemArbitratorSubTasks.CancelStandardModeParse =
        Some(task_counter.WithLabelValues(vec!["cancel-standard-mode-parse"]));
    GlobalMemArbitratorSubTasks.CancelStandardModePlan =
        Some(task_counter.WithLabelValues(vec!["cancel-standard-mode-plan"]));
    GlobalMemArbitratorSubTasks.ForceKillParse =
        Some(task_counter.WithLabelValues(vec!["force-kill-parse"]));
    GlobalMemArbitratorSubTasks.ForceKillPlan =
        Some(task_counter.WithLabelValues(vec!["force-kill-plan"]));
    GlobalMemArbitratorSubTasks.NoLimit = Some(task_counter.WithLabelValues(vec!["nolimit"]));
}

// AddGlobalMemArbitratorCounter 对应 Go 的同名函数，按 taskType 缓存并累加 Counter。
/// 按 taskType 查找或创建 Counter 并累加 count。
pub fn AddGlobalMemArbitratorCounter(
    counterVec: &prometheus::CounterVec,
    taskType: &str,
    count: i64,
) {
    // 读锁只覆盖 map 查询；克隆的是 collector 句柄，不复制底层计数状态。
    let cached = counters
        .read()
        .expect("counter cache read lock poisoned")
        .get(taskType)
        .cloned();

    let counter = match cached {
        Some(counter) => counter,
        None => {
            // 未命中时从 CounterVec 获取对应标签子项，再在写锁下发布到缓存。
            // 与 Go 一样，读锁释放到写锁获取之间允许并发线程创建等价句柄。
            let counter = counterVec.WithLabelValues(vec![taskType]);
            counters
                .write()
                .expect("counter cache write lock poisoned")
                .insert(taskType.to_owned(), counter.clone());
            counter
        }
    };

    counter.Add(count as f64);
}

// SetGlobalMemArbitratorGauge 对应 Go 的同名函数，按 taskType 缓存 Gauge 并设置当前值。
/// 按 taskType 查找或创建 Gauge 并写入当前值。
pub fn SetGlobalMemArbitratorGauge(gaugeVec: &prometheus::GaugeVec, taskType: &str, value: i64) {
    let cached = gauges
        .read()
        .expect("gauge cache read lock poisoned")
        .get(taskType)
        .cloned();

    let gauge = match cached {
        Some(gauge) => gauge,
        None => {
            // 首次出现的 taskType 才访问 GaugeVec，并把句柄存入全局缓存供后续快速复用。
            let gauge = gaugeVec.WithLabelValues(vec![taskType]);
            gauges
                .write()
                .expect("gauge cache write lock poisoned")
                .insert(taskType.to_owned(), gauge.clone());
            gauge
        }
    };

    gauge.Set(value as f64);
}

// ResetGlobalMemArbitratorGauge 对应 Go 的同名函数，把缓存中的所有 task gauge 归零。
/// 将缓存中全部 taskType 对应的 Gauge 重置为 0。
pub fn ResetGlobalMemArbitratorGauge() {
    // Go 使用 RLock 配合 defer RUnlock；这里让只读守卫覆盖整个遍历，防止归零期间 map 结构被写线程修改。
    let cached = gauges.read().expect("gauge cache read lock poisoned");
    for gauge in cached.values() {
        gauge.Set(0.0);
    }
}
