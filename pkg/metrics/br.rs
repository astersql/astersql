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

// BR（Backup & Restore，备份恢复）相关 Prometheus 指标定义与初始化。
//
// 覆盖恢复阶段文件导入、PiTR（Point-in-Time Recovery，时间点恢复）上传、
// Meta KV 批次与 KV apply（将备份 KV 应用到 Region）等观测项。
// Region 是 TiKV 数据分片单位。指标以包级可变静态量保存，对齐 Go 包级变量形状。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 文件导入、PiTR 上传耗时以及建表计数，对应 Go 中最前面的恢复阶段指标。
/// 导入单个文件耗时直方图（含排队）。
pub static mut RestoreImportFileSeconds: Option<prometheus::Histogram> = None;
/// 为 PiTR 上传 SST 文件耗时。
pub static mut RestoreUploadSSTForPiTRSeconds: Option<prometheus::Histogram> = None;
/// 为 PiTR 上传 SST 元数据耗时。
pub static mut RestoreUploadSSTMetaForPiTRSeconds: Option<prometheus::Histogram> = None;
/// 恢复过程中已创建表数量计数。
pub static mut RestoreTableCreatedCount: Option<prometheus::Counter> = None;

// Meta KV 批次指标均带 cf 标签，用于区分 column family。
/// 批次内 Meta KV 文件数（按 cf）。
pub static mut MetaKVBatchFiles: Option<prometheus::HistogramVec> = None;
/// 批次中被过滤的 Meta KV 条目数（按 cf）。
pub static mut MetaKVBatchFilteredKeys: Option<prometheus::HistogramVec> = None;
/// 批次内 Meta KV 条目数（按 cf）。
pub static mut MetaKVBatchKeys: Option<prometheus::HistogramVec> = None;
/// 批次内 Meta KV 总字节数（按 cf）。
pub static mut MetaKVBatchSize: Option<prometheus::HistogramVec> = None;

// KV apply 直方图分别记录批次耗时、文件数、region 数、大小和单 region 文件数。
/// 应用一批 KV 文件的耗时。
pub static mut KVApplyBatchDuration: Option<prometheus::Histogram> = None;
/// 批次内 KV 文件数。
pub static mut KVApplyBatchFiles: Option<prometheus::Histogram> = None;
/// 批次键范围覆盖的 Region 数。
pub static mut KVApplyBatchRegions: Option<prometheus::Histogram> = None;
/// 批次大小（字节量级）。
pub static mut KVApplyBatchSize: Option<prometheus::Histogram> = None;
/// 单个 Region 恢复的 KV 文件数。
pub static mut KVApplyRegionFiles: Option<prometheus::Histogram> = None;

// 事件 CounterVec 的标签语义沿用 Go 注释：event 表示任务/region 事件，status 表示元数据生命周期状态。
/// apply 任务事件计数（标签 event）。
pub static mut KVApplyTasksEvents: Option<prometheus::CounterVec> = None;
/// KV 日志文件元数据内存占用（标签 status）。
pub static mut KVLogFileEmittedMemory: Option<prometheus::CounterVec> = None;
/// 跨 Region 运行事件计数（标签 event）。
pub static mut KVApplyRunOverRegionsEvents: Option<prometheus::CounterVec> = None;
/// split helper 当前内存占用仪表。
pub static mut KVSplitHelperMemUsage: Option<prometheus::Gauge> = None;

// InitBRMetrics 对应 Go 的集中初始化入口。
// 可变静态量仅保留 Go 包级赋值形状；真正接线时需要 OnceLock 等同步初始化，避免并发读写。
/// 初始化全部 BR 指标；持包级锁后写入可变静态量。
pub fn InitBRMetrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    unsafe {
        RestoreTableCreatedCount = Some(metricscommon::NewCounter(prometheus::CounterOpts {
            Namespace: "BR",
            Name: "table_created",
            Help: "The count of tables have been created.",
            ..Default::default()
        }));

        // 文件导入包含下载与排队，因此从 10ms 开始使用较宽的指数桶。
        RestoreImportFileSeconds = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "br",
            Name: "restore_import_file_seconds",
            Help: "The time cost for importing a file. (including the time cost in queuing)",
            Buckets: prometheus::ExponentialBuckets(0.01, 4.0, 14),
        }));
        RestoreUploadSSTForPiTRSeconds =
            Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "br",
                Name: "restore_upload_sst_for_pitr_seconds",
                Help: "The time cost for uploading SST files for point-in-time recovery",
                Buckets: prometheus::DefBuckets,
            }));
        RestoreUploadSSTMetaForPiTRSeconds =
            Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "br",
                Name: "restore_upload_sst_meta_for_pitr_seconds",
                Help: "The time cost for uploading SST metadata for point-in-time recovery",
                Buckets: prometheus::ExponentialBuckets(0.01, 2.0, 14),
            }));

        // 四个 Meta KV 指标保持相同 cf 标签，但桶范围分别匹配文件数、键数和字节数。
        MetaKVBatchFiles = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "br",
                Name: "meta_kv_batch_files",
                Help: "The number of meta KV files in the batch",
                Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 12), // 1 ~ 2048
            },
            &["cf"],
        ));
        MetaKVBatchFilteredKeys = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "br",
                Name: "meta_kv_batch_filtered_keys",
                Help: "The number of filtered meta KV entries from the batch",
                Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 18), // 1 ~ 128Ki
            },
            &["cf"],
        ));
        MetaKVBatchKeys = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "br",
                Name: "meta_kv_batch_keys",
                Help: "The number of meta KV entries in the batch",
                Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 18), // 1 ~ 128Ki
            },
            &["cf"],
        ));
        MetaKVBatchSize = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "br",
                Name: "meta_kv_batch_size",
                Help: "The total size of meta KV entries in the batch",
                Buckets: prometheus::ExponentialBuckets(256.0, 2.0, 20), // 256 ~ 128Mi
            },
            &["cf"],
        ));

        // KV apply 桶边界原样迁移，分别覆盖约 1ms~15min、文件/region 数和 1KiB~1GiB。
        KVApplyBatchDuration = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "br",
            Name: "kv_apply_batch_duration_seconds",
            Help: "The duration to apply the batch of KV files",
            Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 21),
        }));
        KVApplyBatchFiles = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "br",
            Name: "kv_apply_batch_files",
            Help: "The number of KV files in the batch",
            Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 11),
        }));
        KVApplyBatchRegions = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "br",
            Name: "kv_apply_batch_regions",
            Help: "The number of regions in the range of entries in the batch of KV files",
            Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 12),
        }));
        KVApplyBatchSize = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "br",
            Name: "kv_apply_batch_size",
            Help: "The number of KV files in the batch",
            Buckets: prometheus::ExponentialBuckets(1024.0, 2.0, 21),
        }));
        KVApplyRegionFiles = Some(metricscommon::NewHistogram(prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "br",
            Name: "kv_apply_region_files",
            Help: "The number of KV files restored for a region",
            Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 11),
        }));

        // submitted-started 表示排队任务，finished-started 表示运行任务；标签值由调用方提供。
        KVApplyTasksEvents = Some(prometheus::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "br",
                Name: "apply_tasks_events",
                Help: "The count of events of the apply tasks.",
            },
            &["event"],
        ));
        // status 的 0-loaded/1-split/2-applied 差值用于估算不同阶段持有的元数据内存。
        KVLogFileEmittedMemory = Some(prometheus::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "br",
                Name: "kv_log_file_metadata_memory_bytes",
                Help: "The memory usage of metadata for KV log files.",
            },
            &["status"],
        ));
        KVApplyRunOverRegionsEvents = Some(prometheus::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "br",
                Name: "apply_run_over_regions_events",
                Help: "The count of events of the run over regions call.",
            },
            &["event"],
        ));
        KVSplitHelperMemUsage = Some(prometheus::NewGauge(prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "br",
            Name: "kv_split_helper_memory_usage_bytes",
            Help: "The memory usage of the split helper.",
        }));
    }
}
