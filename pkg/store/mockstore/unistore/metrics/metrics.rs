// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// unistore Raft/锁相关 Prometheus Histogram 定义。
//
// 命名空间为 `unistore`、子系统为 `raft`，用于观测 Raft 写路径各阶段等待、
// RaftDB/KVDB/锁更新耗时、latch 等待以及批大小。桶边界与 Go 侧对齐。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::LazyLock;

/// Prometheus 指标命名空间。
const namespace: &str = "unistore";
/// Prometheus 指标子系统（Raft 写入路径）。
const raft: &str = "raft";

/// 构造指数桶 Histogram；help 字符串不能为空（Go 客户端允许空 help）。
fn new_histogram(name: &str, start: f64, factor: f64, count: usize) -> prometheus::Histogram {
    let buckets = prometheus::exponential_buckets(start, factor, count)
        .expect("unistore metric buckets must be valid");
    // The Rust client rejects the empty help string accepted by the Go client.
    // Rust Prometheus 客户端拒绝空 help，故填入固定说明。
    let opts = prometheus::HistogramOpts::new(name, "Unistore raft metric.")
        .namespace(namespace)
        .subsystem(raft)
        .buckets(buckets);
    prometheus::Histogram::with_opts(opts).expect("unistore histogram options must be valid")
}

/// Time spent waiting for the raft writer.
/// Raft writer 队列等待耗时。
pub static RaftWriterWait: LazyLock<prometheus::Histogram> =
    LazyLock::new(|| new_histogram("writer_wait", 0.001, 1.5, 20));
/// Raft 写路径第 1 阶段等待耗时。
pub static WriteWaiteStepOne: LazyLock<prometheus::Histogram> =
    LazyLock::new(|| new_histogram("writer_wait_step_1", 0.001, 1.5, 20));
/// Raft 写路径第 2 阶段等待耗时。
pub static WriteWaiteStepTwo: LazyLock<prometheus::Histogram> =
    LazyLock::new(|| new_histogram("writer_wait_step_2", 0.001, 1.5, 20));
/// Raft 写路径第 3 阶段等待耗时。
pub static WriteWaiteStepThree: LazyLock<prometheus::Histogram> =
    LazyLock::new(|| new_histogram("writer_wait_step_3", 0.001, 1.5, 20));
/// Raft 写路径第 4 阶段等待耗时。
pub static WriteWaiteStepFour: LazyLock<prometheus::Histogram> =
    LazyLock::new(|| new_histogram("writer_wait_step_4", 0.001, 1.5, 20));

/// RaftDB（Raft 日志存储）更新耗时。
pub static RaftDBUpdate: LazyLock<prometheus::Histogram> =
    LazyLock::new(|| new_histogram("raft_db_update", 0.001, 1.5, 20));
/// KVDB（用户键值存储）更新耗时。
pub static KVDBUpdate: LazyLock<prometheus::Histogram> =
    LazyLock::new(|| new_histogram("kv_db_update", 0.001, 1.5, 20));
/// 锁（lockstore）更新耗时。
pub static LockUpdate: LazyLock<prometheus::Histogram> =
    LazyLock::new(|| new_histogram("lock_update", 0.0001, 2.0, 15));
/// Latch（闩锁，用于串行化冲突键写入）等待耗时。
pub static LatchWait: LazyLock<prometheus::Histogram> =
    LazyLock::new(|| new_histogram("latch_wait", 0.0001, 2.0, 15));
/// 单次 Raft 批处理条目数分布。
pub static RaftBatchSize: LazyLock<prometheus::Histogram> =
    LazyLock::new(|| new_histogram("batch_size", 1.0, 1.5, 20));

/// 将 Histogram 注册到默认 Prometheus registry；失败则 panic。
fn must_register(histogram: &LazyLock<prometheus::Histogram>) {
    prometheus::register(Box::new((**histogram).clone())).expect("register unistore metric");
}

/// Registers the metrics related to unistore.
///
/// As with Go's `MustRegister`, duplicate or otherwise invalid registration
/// panics.
/// 注册全部 unistore Raft 相关 Histogram；重复或非法注册会 panic（对齐 Go MustRegister）。
pub fn RegisterMetrics() {
    must_register(&RaftWriterWait);
    must_register(&WriteWaiteStepOne);
    must_register(&WriteWaiteStepTwo);
    must_register(&WriteWaiteStepThree);
    must_register(&WriteWaiteStepFour);
    must_register(&RaftDBUpdate);
    must_register(&KVDBUpdate);
    must_register(&LockUpdate);
    must_register(&RaftBatchSize);
    must_register(&LatchWait);
}
