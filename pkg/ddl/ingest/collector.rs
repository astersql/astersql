// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// DDL ingest（数据导入）过程中的指标收集模块。
//
// ingest 指在 DDL（数据定义语言，如新增索引）执行期间，将索引数据
// 批量导入底层存储的过程。本模块负责按表统计导入过程中的合并
// (merge) 与扫描 (scan) 计数，并记录连接与表的关联关系，供监控使用。

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

pub const METRIC_NAME: &str = "tidb_ddl_temp_index_op_count";
pub const METRIC_HELP: &str = "Gauge of temp index operation count";
pub const LABEL_SINGLE_WRITE: &str = "single_write";
pub const LABEL_DOUBLE_WRITE: &str = "double_write";
pub const LABEL_MERGE: &str = "merge";
pub const LABEL_SCAN: &str = "scan";

/// One Prometheus-compatible snapshot item. `operation` and `table_id`
/// correspond to the Go collector's `type` and `table_id` labels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricSample {
    pub operation: &'static str,
    pub table_id: i64,
    pub value: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct MergeAndScan {
    merge: u64,
    scan: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TableCollector {
    single_write_count: u64,
    double_write_count: u64,
    total_single_write_count: u64,
    total_double_write_count: u64,
}

/// Thread-safe equivalent of the Go package's Prometheus collector.
#[derive(Default)]
pub struct Collector {
    write: Mutex<BTreeMap<u64, BTreeMap<i64, TableCollector>>>,
    read: Mutex<BTreeMap<i64, MergeAndScan>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Go mutexes remain usable after a panic in another goroutine. Recovering
    // poison keeps metrics collection from introducing a Rust-only panic path.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Collector {
    /// Equivalent to `metrics.DDLAddOneTempIndexWrite`.
    pub fn add_temp_index_write(&self, connection_id: u64, table_id: i64, double_write: bool) {
        let mut writes = lock(&self.write);
        let table = writes
            .entry(connection_id)
            .or_default()
            .entry(table_id)
            .or_default();
        if double_write {
            table.double_write_count = table.double_write_count.wrapping_add(1);
        } else {
            table.single_write_count = table.single_write_count.wrapping_add(1);
        }
    }

    /// Commits every pending table count for one connection and clears pending counts.
    pub fn commit_temp_index_write(&self, connection_id: u64) {
        let mut writes = lock(&self.write);
        let Some(connection) = writes.get_mut(&connection_id) else {
            return;
        };
        for table in connection.values_mut() {
            table.total_single_write_count = table
                .total_single_write_count
                .wrapping_add(table.single_write_count);
            table.single_write_count = 0;
            table.total_double_write_count = table
                .total_double_write_count
                .wrapping_add(table.double_write_count);
            table.double_write_count = 0;
        }
    }

    /// Discards every pending table count for one connection.
    pub fn rollback_temp_index_write(&self, connection_id: u64) {
        let mut writes = lock(&self.write);
        let Some(connection) = writes.get_mut(&connection_id) else {
            return;
        };
        for table in connection.values_mut() {
            table.single_write_count = 0;
            table.double_write_count = 0;
        }
    }

    /// Removes one table from every connection and from scan/merge statistics.
    pub fn reset_temp_index_write(&self, table_id: i64) {
        for connection in lock(&self.write).values_mut() {
            connection.remove(&table_id);
        }
        lock(&self.read).remove(&table_id);
    }

    /// Removes all write state belonging to one connection.
    pub fn clear_temp_index_write(&self, connection_id: u64) {
        lock(&self.write).remove(&connection_id);
    }

    /// Accumulates scan and merge counts in the same argument order as Go.
    pub fn set_temp_index_scan_and_merge(&self, table_id: i64, scan_count: u64, merge_count: u64) {
        let mut reads = lock(&self.read);
        let counts = reads.entry(table_id).or_default();
        counts.scan = counts.scan.wrapping_add(scan_count);
        counts.merge = counts.merge.wrapping_add(merge_count);
    }

    /// Takes the values emitted by Go's `Collect`. Pending write counts are not
    /// included until commit, while a known table is still emitted with zero.
    pub fn collect(&self) -> Vec<MetricSample> {
        let mut singles = BTreeMap::<i64, u64>::new();
        let mut doubles = BTreeMap::<i64, u64>::new();
        for connection in lock(&self.write).values() {
            for (&table_id, table) in connection {
                let single = singles.entry(table_id).or_default();
                *single = single.wrapping_add(table.total_single_write_count);
                let double = doubles.entry(table_id).or_default();
                *double = double.wrapping_add(table.total_double_write_count);
            }
        }

        let reads = lock(&self.read);
        let mut samples = Vec::with_capacity((singles.len() + reads.len()) * 2);
        samples.extend(singles.into_iter().map(|(table_id, value)| MetricSample {
            operation: LABEL_SINGLE_WRITE,
            table_id,
            value,
        }));
        samples.extend(doubles.into_iter().map(|(table_id, value)| MetricSample {
            operation: LABEL_DOUBLE_WRITE,
            table_id,
            value,
        }));
        samples.extend(reads.iter().map(|(&table_id, counts)| MetricSample {
            operation: LABEL_MERGE,
            table_id,
            value: counts.merge,
        }));
        samples.extend(reads.iter().map(|(&table_id, counts)| MetricSample {
            operation: LABEL_SCAN,
            table_id,
            value: counts.scan,
        }));
        samples
    }
}
