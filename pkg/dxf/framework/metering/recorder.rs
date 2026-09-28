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

// 单任务计量 Recorder：原子累计对象存储访问与集群流量。
//
// Meter 周期性 scrape `curr_data()` 得到快照，再与上次 flush 比较算增量。

use crate::data::{Data, DataValues};
use recording::{AccessStats, Traffic};
use std::sync::atomic::Ordering;

/// 原子累计对象存储请求与集群流量的单任务 recorder。
/// Recorder accumulates object-store requests and cluster traffic atomically.
#[derive(Default)]
pub struct Recorder {
    /// DXF 任务 ID。
    task_id: i64,
    /// Keyspace（多租户命名空间）。
    keyspace: String,
    /// 任务类型。
    task_type: String,
    /// 对象存储访问统计（请求次数与读写字节）。
    obj_store_access: AccessStats,
    /// 集群读写字节累计。
    cluster_traffic: Traffic,
}

impl Recorder {
    /// 为指定任务构造空计数的 Recorder。
    pub fn new(task_id: i64, keyspace: impl Into<String>, task_type: impl Into<String>) -> Self {
        Self {
            task_id,
            keyspace: keyspace.into(),
            task_type: task_type.into(),
            ..Self::default()
        }
    }

    /// 合并对象存储包装器给出的访问统计快照。
    /// MergeObjStoreAccess merges a snapshot from an object-store wrapper.
    pub fn MergeObjStoreAccess(&self, other: &AccessStats) {
        self.obj_store_access.merge(other);
    }

    /// 累加从集群读取的字节数。
    /// IncClusterReadBytes records bytes read from the cluster.
    pub fn IncClusterReadBytes(&self, n: u64) {
        self.cluster_traffic.read.fetch_add(n, Ordering::Relaxed);
    }

    /// 累加向集群写入的字节数。
    /// IncClusterWriteBytes records bytes written to the cluster.
    pub fn IncClusterWriteBytes(&self, n: u64) {
        self.cluster_traffic.write.fetch_add(n, Ordering::Relaxed);
    }

    /// 测试辅助：累加对象存储 GET 次数。
    #[cfg(test)]
    pub(crate) fn record_obj_store_get(&self, n: u64) {
        self.obj_store_access
            .requests
            .get
            .fetch_add(n, Ordering::Relaxed);
    }

    /// 测试辅助：累加对象存储 PUT 次数。
    #[cfg(test)]
    pub(crate) fn record_obj_store_put(&self, n: u64) {
        self.obj_store_access
            .requests
            .put
            .fetch_add(n, Ordering::Relaxed);
    }

    /// 测试辅助：累加对象存储读字节。
    #[cfg(test)]
    pub(crate) fn record_obj_store_read(&self, n: usize) {
        AccessStats::rec_read(Some(&self.obj_store_access), n);
    }

    /// 测试辅助：累加对象存储写字节。
    #[cfg(test)]
    pub(crate) fn record_obj_store_write(&self, n: usize) {
        AccessStats::rec_write(Some(&self.obj_store_access), n);
    }

    /// 抓取当前累计快照为 `Data`，供 Meter scrape/flush 使用。
    pub(crate) fn curr_data(&self) -> Data {
        Data::new(
            self.task_id,
            &self.keyspace,
            &self.task_type,
            DataValues {
                get_requests: self.obj_store_access.requests.get.load(Ordering::Relaxed),
                put_requests: self.obj_store_access.requests.put.load(Ordering::Relaxed),
                obj_store_read_bytes: self.obj_store_access.traffic.read.load(Ordering::Relaxed),
                obj_store_write_bytes: self.obj_store_access.traffic.write.load(Ordering::Relaxed),
                cluster_read_bytes: self.cluster_traffic.read.load(Ordering::Relaxed),
                cluster_write_bytes: self.cluster_traffic.write.load(Ordering::Relaxed),
            },
        )
    }

    /// 返回用于 Meter 索引的任务 ID。
    pub(crate) fn task_id_for_meter(&self) -> i64 {
        self.task_id
    }
}
