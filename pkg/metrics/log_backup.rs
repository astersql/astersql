// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 日志备份（log backup）相关 Prometheus 指标定义与初始化。
//
// 覆盖全局 checkpoint、外部存储落盘进度、advancer 所有者与 tick 耗时、
// Region checkpoint 请求成败，以及当前任务中 checkpoint 最小的 Region 信息。
// Region 是 TiKV 的数据分片单位；checkpoint 表示备份可安全回放到的时间点。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 日志备份指标的详细含义仍以各项 Help 文本为准；Option 表达 Go 全局量初始化前的空状态。
/// 各任务最近一次全局 checkpoint 水位。
pub static mut LastCheckpoint: Option<prometheus::GaugeVec> = None;
/// 已成功持久化到外部存储的全局 checkpoint。
pub static mut ExternalStorageCheckpoint: Option<prometheus::GaugeVec> = None;
/// 本节点是否为 advancer 所有者（1 是 / 0 否）。
pub static mut AdvancerOwner: Option<prometheus::Gauge> = None;
/// advancer 各步骤 tick 耗时分布（秒）。
pub static mut AdvancerTickDuration: Option<prometheus::HistogramVec> = None;
/// 扫描 Region 或拉取 Region checkpoint 的批大小分布。
pub static mut GetCheckpointBatchSize: Option<prometheus::HistogramVec> = None;
/// Region checkpoint 请求成功 / 失败计数。
pub static mut RegionCheckpointRequest: Option<prometheus::CounterVec> = None;
/// Region checkpoint 请求失败原因计数。
pub static mut RegionCheckpointFailure: Option<prometheus::CounterVec> = None;
/// Region flush / 订阅事件规模分布。
pub static mut RegionCheckpointSubscriptionEvent: Option<prometheus::HistogramVec> = None;

// 这两个 gauge 保留当前任务中 checkpoint 最小的 region 及其 leader store 标识。
/// 当前任务中 checkpoint ts 最小的 Region ID。
pub static mut LogBackupCurrentLastRegionID: Option<prometheus::Gauge> = None;
/// 上述 Region 的 leader 所在 store ID。
pub static mut LogBackupCurrentLastRegionLeaderStoreID: Option<prometheus::Gauge> = None;

// InitLogBackupMetrics 对应 Go 的同名函数，按原顺序构造 checkpoint、advancer 与 region 指标。
// 可变全局赋值只是机械保留 Go 包级初始化形状；真实 Rust 接线需要线程安全的一次初始化机制。
/// 初始化本文件全部日志备份指标；须在持有包级初始化锁的前提下调用。
pub unsafe fn InitLogBackupMetrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    LastCheckpoint = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "log_backup",
            Name: "last_checkpoint",
            Help: "The last global checkpoint of log backup.",
            ..Default::default()
        },
        vec!["task"],
    ));

    ExternalStorageCheckpoint = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "log_backup",
            Name: "external_storage_checkpoint",
            Help: "The global checkpoint that has been successfully persisted to external storage.",
            ..Default::default()
        },
        vec!["task"],
    ));

    AdvancerOwner = Some(metricscommon::NewGauge(prometheus::GaugeOpts {
        Namespace: "tidb",
        Subsystem: "log_backup",
        Name: "advancer_owner",
        Help: "If the node is the owner of advancers, set this to `1`, otherwise `0`.",
        // Go 显式传入空 ConstLabels map；保留“无固定标签”而非省略字段的语义。
        ..Default::default()
    }));

    AdvancerTickDuration = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "log_backup",
            Name: "advancer_tick_duration_sec",
            Help: "The time cost of each step during advancer ticking.",
            Buckets: prometheus::ExponentialBuckets(0.01, 3.0, 8),
            ..Default::default()
        },
        vec!["step"],
    ));

    GetCheckpointBatchSize = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "log_backup",
            Name: "advancer_batch_size",
            Help: "The batch size of scanning region or get region checkpoint.",
            Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 12),
            ..Default::default()
        },
        vec!["type"],
    ));

    RegionCheckpointRequest = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "log_backup",
            Name: "region_request",
            Help: "The failure / success stat requesting region checkpoints.",
            ..Default::default()
        },
        vec!["result"],
    ));

    RegionCheckpointFailure = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "log_backup",
            Name: "region_request_failure",
            Help: "The failure reasons of requesting region checkpoints.",
            ..Default::default()
        },
        vec!["reason"],
    ));

    RegionCheckpointSubscriptionEvent = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "log_backup",
            Name: "region_checkpoint_event",
            Help: "The region flush event size.",
            Buckets: prometheus::ExponentialBuckets(8.0, 2.0, 12),
            ..Default::default()
        },
        vec!["store"],
    ));

    LogBackupCurrentLastRegionID = Some(metricscommon::NewGauge(prometheus::GaugeOpts {
        Namespace: "tidb",
        Subsystem: "log_backup",
        Name: "current_last_region_id",
        Help: "The id of the region have minimal checkpoint ts in the current running task.",
        ..Default::default()
    }));

    LogBackupCurrentLastRegionLeaderStoreID = Some(metricscommon::NewGauge(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "log_backup",
            Name: "current_last_region_leader_store_id",
            Help: "The leader's store id of the region have minimal checkpoint ts in the current running task.",
            ..Default::default()
        },
    ));
}
