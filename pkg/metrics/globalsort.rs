// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 全局排序（Global Sort）与归并排序相关 Prometheus 指标。
//
// 全局排序常用于加索引等需要跨节点有序数据的场景：将中间结果写入云存储后再读回，
// 由 ingest/upload worker 并行处理。本模块统计读写云存储的耗时与速率、worker 数量，
// 以及归并排序的读写字节数。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 下列 Option 对应 Go 中初始化前为 nil 的指针或零值接口；初始化函数负责一次性填入具体 collector。
/// 写入云存储耗时直方图。
// GlobalSortWriteToCloudStorageDuration 记录写入云存储的耗时。
pub static mut GlobalSortWriteToCloudStorageDuration: Option<prometheus::HistogramVec> = None;
/// 写入云存储速率直方图。
// GlobalSortWriteToCloudStorageRate 记录写入云存储的速率。
pub static mut GlobalSortWriteToCloudStorageRate: Option<prometheus::HistogramVec> = None;
/// 从云存储读取耗时直方图。
// GlobalSortReadFromCloudStorageDuration 记录从云存储读取的耗时。
pub static mut GlobalSortReadFromCloudStorageDuration: Option<prometheus::HistogramVec> = None;
/// 从云存储读取速率直方图。
// GlobalSortReadFromCloudStorageRate 记录从云存储读取的速率。
pub static mut GlobalSortReadFromCloudStorageRate: Option<prometheus::HistogramVec> = None;
/// 正在工作的 ingest worker 数量。
// GlobalSortIngestWorkerCnt 记录正在工作的 ingest worker 数量。
pub static mut GlobalSortIngestWorkerCnt: Option<prometheus::GaugeVec> = None;
/// 活跃并行上传 worker 数量。
// GlobalSortUploadWorkerCount 记录活跃并行上传 worker 数量。
pub static mut GlobalSortUploadWorkerCount: Option<prometheus::Gauge> = None;
/// 归并排序写出字节数。
// MergeSortWriteBytes 记录归并排序写出的字节数。
pub static mut MergeSortWriteBytes: Option<prometheus::Counter> = None;
/// 归并排序读入字节数。
// MergeSortReadBytes 记录归并排序读入的字节数。
pub static mut MergeSortReadBytes: Option<prometheus::Counter> = None;

/// 初始化全局排序与归并排序相关全部指标 collector。
// InitGlobalSortMetrics 对应 Go 的同名初始化函数，逐个构造全局排序 collector。
// 对可变全局量的赋值只保留 Go 包级初始化形状；真实 Rust 接线需由线程安全的一次性容器承载。
pub unsafe fn InitGlobalSortMetrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    GlobalSortWriteToCloudStorageDuration = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "global_sort",
            Name: "write_to_cloud_storage_duration",
            Help: "write to cloud storage duration",
            // 与 Go 一致：从 1ms 开始按 2 倍增长，共 20 个桶，覆盖约 524 秒。
            Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 20),
            ..Default::default()
        },
        vec![LblType],
    ));

    GlobalSortWriteToCloudStorageRate = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "global_sort",
            Name: "write_to_cloud_storage_rate",
            Help: "write to cloud storage rate",
            Buckets: prometheus::ExponentialBuckets(0.05, 2.0, 20),
            ..Default::default()
        },
        vec![LblType],
    ));

    GlobalSortReadFromCloudStorageDuration = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "global_sort",
            Name: "read_from_cloud_storage_duration",
            Help: "read from cloud storage duration",
            Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 20),
            ..Default::default()
        },
        vec![LblType],
    ));

    GlobalSortReadFromCloudStorageRate = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "global_sort",
            Name: "read_from_cloud_storage_rate",
            Help: "read from cloud storage rate",
            Buckets: prometheus::ExponentialBuckets(0.05, 2.0, 20),
            ..Default::default()
        },
        vec![LblType],
    ));

    GlobalSortIngestWorkerCnt = Some(metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "global_sort",
            Name: "ingest_worker_cnt",
            Help: "ingest worker cnt",
            ..Default::default()
        },
        vec![LblType],
    ));

    GlobalSortUploadWorkerCount = Some(metricscommon::NewGauge(prometheus::GaugeOpts {
        Namespace: "tidb",
        Subsystem: "global_sort",
        Name: "upload_worker_cnt",
        Help: "Gauge of active parallel upload worker count.",
        ..Default::default()
    }));

    MergeSortWriteBytes = Some(metricscommon::NewCounter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: "global_sort",
        Name: "merge_sort_write_bytes",
        Help: "Counter of bytes written in merge sort.",
        ..Default::default()
    }));

    MergeSortReadBytes = Some(metricscommon::NewCounter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: "global_sort",
        Name: "merge_sort_read_bytes",
        Help: "Counter of bytes read in merge sort.",
        ..Default::default()
    }));
}
