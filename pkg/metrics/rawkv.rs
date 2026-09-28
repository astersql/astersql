// Copyright 2025 PingCAP, Inc.
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

// RawKV（原始键值）批量写入相关 Prometheus 指标。
//
// 记录 batch put 的耗时与批内条目数分布；均按 column family（cf）标签隔离观测。
// RawKV 绕过 SQL 层直接访问 TiKV 键值接口，常用于导入或内部批量写入路径。

use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};

// RawKVBatchPutDurationSeconds records the time cost for batch put.
// Option 对应 Go 指针在 InitRawKVMetrics 调用前的 nil 状态。
/// RawKV batch put 耗时直方图（秒），标签为 cf。
pub static mut RAW_KV_BATCH_PUT_DURATION_SECONDS: Option<prometheus::HistogramVec> = None;

// RawKVBatchPutBatchSize records the number of kv entries in the batch put.
/// RawKV batch put 批内键值条目数直方图，标签为 cf。
pub static mut RAW_KV_BATCH_PUT_BATCH_SIZE: Option<prometheus::HistogramVec> = None;

// init_raw_kv_metrics 对应 Go 的 InitRawKVMetrics：构造耗时与批大小两个直方图。
// 两项指标都以 cf 为唯一标签，使不同 column family 的观测值保持隔离。
/// 初始化 RawKV batch put 耗时与批大小指标。
pub unsafe fn init_raw_kv_metrics() {
    RAW_KV_BATCH_PUT_DURATION_SECONDS = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "rawkv",
            Name: "rawkv_batch_put_duration_seconds",
            Help: "The time cost batch put kvs",
            // 1ms 起按 2 倍增长 17 桶，覆盖到约 1 分钟。
            Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 17),
        },
        &["cf"],
    ));

    RAW_KV_BATCH_PUT_BATCH_SIZE = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "rawkv",
            Name: "rawkv_batch_put_batch_size",
            Help: "Number of kv entries in the batch put",
            // 保留 Go 的 1、2、4……256 共 9 个桶。
            Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 9),
        },
        &["cf"],
    ));
}
