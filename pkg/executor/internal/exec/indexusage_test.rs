// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 索引使用情况上报器的单元测试。
//
// 通过内存收集器和轻量模拟对象，验证聚簇索引选择、物理表统计过滤、
// Point Get 缺省行数，以及有符号计数转换等行为与 Go 实现保持一致。

use std::collections::HashMap;
use std::sync::Arc;

use crate::indexusage::{
    InMemoryIndexUsageCollector, IndexInfo, IndexUsageReporter, PSEUDO_VERSION,
    RuntimeStatsCollection, Table, TableInfo, TableStats, UsedStatsInfo,
};

/// 覆盖整型主键句柄、公共句柄和隐藏 row ID 三类表的聚簇索引判定。
#[test]
fn cluster_index_selection_matches_go_for_all_table_kinds() {
    let integer_handle = TableInfo {
        pk_is_handle: true,
        ..Default::default()
    };
    assert_eq!(
        crate::indexusage::getClusterIndexID(&integer_handle),
        Some(0)
    );

    let common_handle = TableInfo {
        is_common_handle: true,
        indices: vec![IndexInfo {
            id: 9,
            primary: true,
        }],
        ..Default::default()
    };
    assert_eq!(
        crate::indexusage::getClusterIndexID(&common_handle),
        Some(9)
    );

    // 即使元数据缺少主索引，Go 对公共句柄表仍返回 `(0, true)`；这里保留索引 ID 0，
    // 避免静默丢弃该表的索引使用记录。
    let incomplete_common_handle = TableInfo {
        is_common_handle: true,
        indices: vec![IndexInfo {
            id: 9,
            primary: false,
        }],
        ..Default::default()
    };
    assert_eq!(
        crate::indexusage::getClusterIndexID(&incomplete_common_handle),
        Some(0)
    );

    let rowid_table = TableInfo::default();
    assert_eq!(crate::indexusage::getClusterIndexID(&rowid_table), None);
}

/// 按执行计划 ID 提供 Coprocessor 请求数和访问行数。
#[derive(Default)]
struct RuntimeStatsMock {
    values: HashMap<i32, (i64, i64)>,
}

impl RuntimeStatsCollection for RuntimeStatsMock {
    fn GetCopCountAndRows(&self, plan_id: i32) -> (i64, i64) {
        self.values.get(&plan_id).copied().unwrap_or_default()
    }
}

/// 按物理表 ID 提供本次语句实际使用的统计快照。
#[derive(Default)]
struct UsedStatsMock {
    values: HashMap<i64, TableStats>,
}

impl UsedStatsInfo for UsedStatsMock {
    fn GetUsedInfo(&self, table_id: i64) -> Option<TableStats> {
        self.values.get(&table_id).copied()
    }
}

/// 同时携带逻辑表元信息和可选分区物理表 ID 的测试表。
struct TableMock {
    meta: TableInfo,
    physical_id: Option<i64>,
}

impl Table for TableMock {
    fn Meta(&self) -> &TableInfo {
        &self.meta
    }

    fn GetPhysicalID(&self) -> Option<i64> {
        self.physical_id
    }
}

/// 构造共享收集器及包含有效、伪版本和异常运行时计数的固定测试夹具。
fn reporter() -> (IndexUsageReporter, Arc<InMemoryIndexUsageCollector>) {
    let collector = Arc::new(InMemoryIndexUsageCollector::default());
    let runtime = Arc::new(RuntimeStatsMock {
        values: HashMap::from([(7, (3, 11)), (8, (0, 0)), (9, (-1, -2))]),
    });
    let stats = Arc::new(UsedStatsMock {
        values: HashMap::from([
            (
                20,
                TableStats {
                    version: 1,
                    realtime_count: 100,
                },
            ),
            (
                21,
                TableStats {
                    version: PSEUDO_VERSION,
                    realtime_count: 100,
                },
            ),
        ]),
    });
    (
        IndexUsageReporter::NewIndexUsageReporter(collector.clone(), runtime, Some(stats)),
        collector,
    )
}

/// Coprocessor 上报按物理表读取统计，并跳过零访问或伪统计样本。
#[test]
fn cop_report_uses_physical_table_stats_and_skips_zero_or_pseudo_stats() {
    let (reporter, collector) = reporter();
    let table = TableMock {
        meta: TableInfo {
            id: 10,
            is_common_handle: true,
            indices: vec![IndexInfo {
                id: 7,
                primary: true,
            }],
            ..Default::default()
        },
        physical_id: Some(20),
    };

    reporter.ReportCopIndexUsageForHandle(&table, 7);
    assert_eq!(
        collector.samples(10, 7),
        vec![crate::indexusage::IndexUsageSample {
            query_total: 0,
            kv_request_total: 3,
            rows: 11,
            table_rows: 100,
        }]
    );

    reporter.ReportCopIndexUsage(10, 21, 7, 7);
    reporter.ReportCopIndexUsage(10, 20, 7, 8);
    assert_eq!(collector.samples(10, 7).len(), 1);
}

/// Point Get 缺少或只有伪统计时使用最大有符号 32 位整数，并继续上报使用量。
#[test]
fn point_get_falls_back_to_max_i32_without_real_stats_and_preserves_go_casts() {
    let (reporter, collector) = reporter();

    reporter.ReportPointGetIndexUsage(10, 999, 7, 1, 2);
    let sample = collector.samples(10, 7)[0];
    assert_eq!(sample.table_rows, i32::MAX as u64);
    assert_eq!(sample.kv_request_total, 1);
    assert_eq!(sample.rows, 2);

    reporter.ReportPointGetIndexUsage(10, 21, 7, 1, 2);
    let sample = collector.samples(10, 7)[1];
    assert_eq!(sample.table_rows, i32::MAX as u64);
    assert_eq!(sample.kv_request_total, 1);
    assert_eq!(sample.rows, 2);

    reporter.ReportCopIndexUsage(10, 20, 7, 9);
    let sample = collector.samples(10, 7)[2];
    assert_eq!(sample.kv_request_total, u64::MAX);
    assert_eq!(sample.rows, u64::MAX - 1);
}

/// 零请求且零访问不产生样本，表行数也只接受非伪版本统计。
#[test]
fn runtime_stats_and_table_row_count_follow_go_zero_access_rules() {
    let (reporter, collector) = reporter();
    reporter.ReportCopIndexUsage(10, 20, 7, 8);
    assert!(collector.samples(10, 7).is_empty());
    assert_eq!(reporter.getTableRowCount(20), Some(100));
    assert_eq!(reporter.getTableRowCount(21), None);
    assert_eq!(reporter.getTableRowCount(999), None);
}
