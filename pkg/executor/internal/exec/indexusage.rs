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

// 索引使用情况上报（Index Usage Reporter）。
//
// 在执行器访问索引（Coprocessor 扫描或 Point Get）后，把请求次数、访问行数、表行数
// 汇总到语句级收集器，供统计/优化器评估索引热度。伪统计版本（`PSEUDO_VERSION`）
// 表示缺少真实表统计，此时 Cop 路径会跳过上报。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 伪统计版本号：`version == 0` 表示未使用真实表统计信息。
pub const PSEUDO_VERSION: u64 = 0;

/// 索引元信息：标识与是否为主键索引。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexInfo {
    pub id: i64,
    pub primary: bool,
}

/// 表元信息：含聚簇索引相关标志与索引列表。
///
/// `pk_is_handle` 表示整型主键即行句柄；`is_common_handle` 表示主键为公共句柄
/// （非整数主键的聚簇索引）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    pub id: i64,
    pub pk_is_handle: bool,
    pub is_common_handle: bool,
    pub indices: Vec<IndexInfo>,
}

/// 表抽象：提供元信息与可选物理表 ID（分区表场景）。
pub trait Table: Send + Sync {
    fn Meta(&self) -> &TableInfo;
    fn GetPhysicalID(&self) -> Option<i64> {
        None
    }
}

/// 表级统计快照：版本与实时行数估计。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TableStats {
    pub version: u64,
    pub realtime_count: i64,
}

/// 查询语句实际使用到的表统计信息映射。
pub trait UsedStatsInfo: Send + Sync {
    fn GetUsedInfo(&self, table_id: i64) -> Option<TableStats>;
}
/// 运行时统计：按 plan_id 取 Coprocessor 请求次数与访问行数。
pub trait RuntimeStatsCollection: Send + Sync {
    fn GetCopCountAndRows(&self, plan_id: i32) -> (i64, i64);
}

/// 单次索引访问采样：查询次数、KV 请求数、访问行数、表总行数。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IndexUsageSample {
    pub query_total: u64,
    pub kv_request_total: u64,
    pub rows: u64,
    pub table_rows: u64,
}

/// 语句级索引使用收集器接口。
pub trait StmtIndexUsageCollector: Send + Sync {
    fn Update(&self, table_id: i64, index_id: i64, sample: IndexUsageSample);
}

/// 内存实现的收集器，便于单测断言采样内容。
#[derive(Default)]
pub struct InMemoryIndexUsageCollector {
    samples: Mutex<HashMap<(i64, i64), Vec<IndexUsageSample>>>,
}
impl StmtIndexUsageCollector for InMemoryIndexUsageCollector {
    fn Update(&self, table_id: i64, index_id: i64, sample: IndexUsageSample) {
        if let Ok(mut samples) = self.samples.lock() {
            samples
                .entry((table_id, index_id))
                .or_default()
                .push(sample);
        }
    }
}
impl InMemoryIndexUsageCollector {
    /// 返回指定表/索引上已记录的全部采样。
    pub fn samples(&self, table_id: i64, index_id: i64) -> Vec<IndexUsageSample> {
        self.samples
            .lock()
            .ok()
            .and_then(|samples| samples.get(&(table_id, index_id)).cloned())
            .unwrap_or_default()
    }
}

/// 将执行器侧索引访问转化为 `IndexUsageSample` 并写入收集器。
pub struct IndexUsageReporter {
    reporter: Arc<dyn StmtIndexUsageCollector>,
    runtime_stats: Arc<dyn RuntimeStatsCollection>,
    stats_map: Option<Arc<dyn UsedStatsInfo>>,
}

impl IndexUsageReporter {
    /// 构造上报器；`stats_map` 为空时 Point Get 仍可用默认表行数兜底。
    pub fn NewIndexUsageReporter(
        reporter: Arc<dyn StmtIndexUsageCollector>,
        runtime_stats: Arc<dyn RuntimeStatsCollection>,
        stats_map: Option<Arc<dyn UsedStatsInfo>>,
    ) -> Self {
        Self {
            reporter,
            runtime_stats,
            stats_map,
        }
    }

    /// 针对表句柄/聚簇索引上报 Coprocessor 路径的索引使用。
    pub fn ReportCopIndexUsageForHandle(&self, table: &dyn Table, plan_id: i32) {
        let Some(index_id) = getClusterIndexID(table.Meta()) else {
            return;
        };
        self.ReportCopIndexUsageForTable(table, index_id, plan_id);
    }

    /// 按逻辑表解析物理表 ID 后上报 Cop 索引使用。
    pub fn ReportCopIndexUsageForTable(&self, table: &dyn Table, index_id: i64, plan_id: i32) {
        let table_id = table.Meta().id;
        self.ReportCopIndexUsage(
            table_id,
            table.GetPhysicalID().unwrap_or(table_id),
            index_id,
            plan_id,
        );
    }

    /// 从运行时统计取 Cop 请求/行数，结合表行数写入采样；无有效统计或零访问则跳过。
    pub fn ReportCopIndexUsage(
        &self,
        table_id: i64,
        physical_table_id: i64,
        index_id: i64,
        plan_id: i32,
    ) {
        let Some(table_rows) = self.getTableRowCount(physical_table_id) else {
            return;
        };
        let (kv_requests, access_rows) = self.runtime_stats.GetCopCountAndRows(plan_id);
        if kv_requests == 0 && access_rows == 0 {
            return;
        }
        self.reporter.Update(
            table_id,
            index_id,
            IndexUsageSample {
                query_total: 0,
                kv_request_total: goUint64(kv_requests),
                rows: goUint64(access_rows),
                table_rows: goUint64(table_rows),
            },
        );
    }

    /// Point Get 路径：针对主键句柄上报索引使用。
    pub fn ReportPointGetIndexUsageForHandle(
        &self,
        table: &TableInfo,
        physical_table_id: i64,
        kv_requests: i64,
        rows: i64,
    ) {
        let Some(index_id) = getClusterIndexID(table) else {
            return;
        };
        self.ReportPointGetIndexUsage(table.id, physical_table_id, index_id, kv_requests, rows);
    }

    /// Point Get 路径上报；无表统计时用 `i32::MAX` 作表行数，使任意非零访问落入最小非零百分比桶。
    pub fn ReportPointGetIndexUsage(
        &self,
        table_id: i64,
        physical_table_id: i64,
        index_id: i64,
        kv_requests: i64,
        rows: i64,
    ) {
        // Point gets may run without table statistics. Go uses MaxInt32 so any
        // non-zero access is assigned to the smallest non-zero percentage bucket.
        let table_rows = self
            .getTableRowCount(physical_table_id)
            .unwrap_or(i32::MAX as i64);
        self.reporter.Update(
            table_id,
            index_id,
            IndexUsageSample {
                query_total: 0,
                kv_request_total: goUint64(kv_requests),
                rows: goUint64(rows),
                table_rows: goUint64(table_rows),
            },
        );
    }

    /// 读取物理表的实时行数；伪版本统计视为不可用。
    pub fn getTableRowCount(&self, table_id: i64) -> Option<i64> {
        let stats = self.stats_map.as_ref()?.GetUsedInfo(table_id)?;
        (stats.version != PSEUDO_VERSION).then_some(stats.realtime_count)
    }
}

/// Match Go's `uint64(int64)` conversion used by `indexusage.NewSample`.
fn goUint64(value: i64) -> u64 {
    value as u64
}

/// 解析聚簇索引 ID：整型主键句柄用 0；公共句柄取主键索引 ID；否则无索引可报。
pub fn getClusterIndexID(table: &TableInfo) -> Option<i64> {
    if table.pk_is_handle {
        return Some(0);
    }
    if table.is_common_handle {
        return Some(
            table
                .indices
                .iter()
                .find(|index| index.primary)
                .map(|index| index.id)
                .unwrap_or(0),
        );
    }
    None
}
