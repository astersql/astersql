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

//! 索引使用情况的采样、聚合与会话上报。
//!
//! 会话收集器先按表和索引累计查询次数、KV 请求数、扫描行数及访问比例直方图，
//! 再通过 `report` 或 `flush` 合并到节点级收集器，供统计信息接口查询和清理。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// 单个索引的使用样本，与 Go 的 `indexusage.Sample` 保持一致。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexUsageSample {
    /// 最近一次使用该索引的时间。
    pub last_used_at: SystemTime,
    /// 使用该索引的查询总数。
    pub query_total: u64,
    /// 读取该索引所发送的 KV 请求总数。
    pub kv_req_total: u64,
    /// 通过该索引扫描的行数总和。
    pub row_access_total: u64,
    /// 扫描行数占全表行数比例的七桶直方图：0、0～1%、1%～10%、
    /// 10%～20%、20%～50%、50%～100% 和 100%。
    pub percentage_access: [u64; 7],
}

impl Default for IndexUsageSample {
    fn default() -> Self {
        Self {
            last_used_at: SystemTime::UNIX_EPOCH
                .checked_sub(Duration::from_secs(62_135_596_800))
                .expect("SystemTime must represent Go's zero time"),
            query_total: 0,
            kv_req_total: 0,
            row_access_total: 0,
            percentage_access: [0; 7],
        }
    }
}

const BUCKET_BOUND: [f64; 6] = [0.0, 0.01, 0.1, 0.2, 0.5, 1.0];

fn access_bucket(percentage: f64) -> usize {
    if percentage == 0.0 {
        return 0;
    }
    for i in 1..BUCKET_BOUND.len() {
        if percentage >= BUCKET_BOUND[i - 1] && percentage < BUCKET_BOUND[i] {
            return i;
        }
    }
    if percentage == 1.0 {
        return BUCKET_BOUND.len();
    }
    // Go 实现会让大于 1.0 的异常比例因未命中区间而保留在第 0 桶；这里保持该行为。
    0
}

/// 创建一个数据点，并在访问比例直方图中恰好记录一个桶。
///
/// 当全表行数为 0 时按访问比例 100% 处理，直接计入末桶，避免除零。
pub fn new_sample(
    query_total: u64,
    kv_req_total: u64,
    row_access: u64,
    table_total_rows: u64,
) -> IndexUsageSample {
    let bucket = if table_total_rows == 0 {
        BUCKET_BOUND.len()
    } else {
        access_bucket(row_access as f64 / table_total_rows as f64)
    };
    let mut percentage_access = [0; 7];
    percentage_access[bucket] = 1;
    IndexUsageSample {
        last_used_at: SystemTime::now(),
        query_total,
        kv_req_total,
        row_access_total: row_access,
        percentage_access,
    }
}

/// 保留 Go 风格命名，供按源 API 生成的调用方兼容使用。
#[allow(non_snake_case)]
pub fn NewSample(
    query_total: u64,
    kv_req_total: u64,
    row_access: u64,
    table_total_rows: u64,
) -> IndexUsageSample {
    new_sample(query_total, kv_req_total, row_access, table_total_rows)
}

type IndexKey = (i64, i64);

fn merge_sample(target: &mut IndexUsageSample, incoming: &IndexUsageSample) {
    // 与 Go 的无符号整数累加语义一致：溢出时回绕，而不是在调试构建中触发 panic。
    target.query_total = target.query_total.wrapping_add(incoming.query_total);
    target.kv_req_total = target.kv_req_total.wrapping_add(incoming.kv_req_total);
    target.row_access_total = target
        .row_access_total
        .wrapping_add(incoming.row_access_total);
    for (current, added) in target
        .percentage_access
        .iter_mut()
        .zip(incoming.percentage_access)
    {
        *current = current.wrapping_add(added);
    }
    if target.last_used_at < incoming.last_used_at {
        target.last_used_at = incoming.last_used_at;
    }
}

/// 节点级索引使用收集器。
///
/// 会话增量只有在调用 `report` 或 `flush` 后才对全局查询可见，对应 Go 收集器
/// 将增量发送给后台工作线程后的可见时机。
#[derive(Clone, Default)]
pub struct IndexUsageCollector {
    samples: Arc<Mutex<HashMap<IndexKey, IndexUsageSample>>>,
    running: Arc<AtomicBool>,
}

impl IndexUsageCollector {
    /// 直接写入节点级映射，便于需要确定性结果的测试或调用方使用。
    pub fn record(&self, table_id: i64, index_id: i64, sample: IndexUsageSample) {
        let mut samples = self.samples.lock().expect("index usage mutex poisoned");
        merge_sample(
            samples
                .entry((table_id, index_id))
                .or_insert_with(IndexUsageSample::default),
            &sample,
        );
    }

    /// 兼容旧版四参数本地 API；缺失的 KV 请求数按 0 处理。
    pub fn record_counts(&self, table_id: i64, index_id: i64, query_total: u64, row_access: u64) {
        self.record(
            table_id,
            index_id,
            new_sample(query_total, 0, row_access, row_access),
        );
    }

    pub fn sample(&self, table_id: i64, index_id: i64) -> IndexUsageSample {
        self.samples
            .lock()
            .expect("index usage mutex poisoned")
            .get(&(table_id, index_id))
            .cloned()
            .unwrap_or_default()
    }

    pub fn start_worker(&self) {
        self.running.store(true, Ordering::Release);
    }

    pub fn close(&self) {
        self.running.store(false, Ordering::Release);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn spawn_session(&self) -> SessionIndexUsageCollector {
        SessionIndexUsageCollector {
            global: self.clone(),
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 删除表或索引已不存在的使用记录。
    pub fn gc<F>(&self, mut index_exists: F)
    where
        F: FnMut(i64, i64) -> bool,
    {
        self.samples
            .lock()
            .expect("index usage mutex poisoned")
            .retain(|&(table_id, index_id), _| index_exists(table_id, index_id));
    }

    fn merge_pending(&self, pending: &mut HashMap<IndexKey, IndexUsageSample>) {
        // drain 同时转移并清空会话增量，确保一次上报不会被重复累计。
        let mut samples = self.samples.lock().expect("index usage mutex poisoned");
        for (key, incoming) in pending.drain() {
            merge_sample(
                samples.entry(key).or_insert_with(IndexUsageSample::default),
                &incoming,
            );
        }
    }
}

/// 会话级索引使用收集器。
///
/// Go 的 `Report` 采用非阻塞通道；此内存实现没有有界通道，因此 `report` 与
/// `flush` 都会立即合并，且不会因通道已满而丢弃样本。
#[derive(Clone)]
pub struct SessionIndexUsageCollector {
    global: IndexUsageCollector,
    pending: Arc<Mutex<HashMap<IndexKey, IndexUsageSample>>>,
}

impl SessionIndexUsageCollector {
    /// 将一个样本累计到当前会话的待上报增量中。
    pub fn update(&self, table_id: i64, index_id: i64, sample: IndexUsageSample) {
        let mut pending = self.pending.lock().expect("index usage mutex poisoned");
        merge_sample(
            pending
                .entry((table_id, index_id))
                .or_insert_with(IndexUsageSample::default),
            &sample,
        );
    }

    /// 将当前会话的全部待上报增量合并到节点级收集器。
    pub fn report(&self) {
        let mut pending = self.pending.lock().expect("index usage mutex poisoned");
        self.global.merge_pending(&mut pending);
    }

    /// 同步上报当前会话增量；内存实现中与 `report` 等价。
    pub fn flush(&self) {
        self.report();
    }

    /// 查询尚未上报的会话级样本；不存在时返回 `None`。
    pub fn sample(&self, table_id: i64, index_id: i64) -> Option<IndexUsageSample> {
        self.pending
            .lock()
            .expect("index usage mutex poisoned")
            .get(&(table_id, index_id))
            .cloned()
    }
}

impl crate::StatsUsageImpl {
    /// 创建一个绑定到当前节点级收集器的会话收集器。
    pub fn new_session_index_usage_collector(&self) -> SessionIndexUsageCollector {
        self.index_usage.spawn_session()
    }

    /// 委托持久层清理已失效的索引使用记录。
    pub fn gc_index_usage(&self) -> Result<(), crate::Error> {
        self.store.gc_index_usage()
    }

    /// 标记节点级索引使用收集器为运行状态。
    pub fn start_worker(&self) {
        self.index_usage.start_worker();
    }

    /// 停止节点级索引使用收集器。
    pub fn close(&self) {
        self.index_usage.close();
    }

    /// 获取指定表和索引的节点级聚合样本，不存在时返回空样本。
    pub fn get_index_usage(&self, table_id: i64, index_id: i64) -> IndexUsageSample {
        self.index_usage.sample(table_id, index_id)
    }
}
