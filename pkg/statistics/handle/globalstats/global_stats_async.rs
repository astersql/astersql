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

// 分区统计异步合并任务封装。
//
// 对应 Go 侧异步合并全局统计（global stats）的任务对象：持有分区统计提供者、
// 表 ID 与列/索引项数量，调用 `merge_partition_stats_to_global` 并将结果与
// 缺失分区列表缓存下来。`cancelled` 用于协作式取消。

use crate::{GlobalStats, MergeOptions, PartitionStatsProvider, merge_partition_stats_to_global};
use std::sync::atomic::{AtomicBool, Ordering};

/// 一次异步合并作业的状态：输入参数、合并结果与缺失分区名列表。
pub struct AsyncMergePartitionStats<'a> {
    /// 分区统计数据源。
    provider: &'a dyn PartitionStatsProvider,
    /// 分区表的全局表 ID。
    table_id: i64,
    /// 待合并的直方图项数量（列/索引）。
    item_count: usize,
    /// 合并完成后的全局统计；未执行时为 `None`。
    result: Option<GlobalStats>,
    /// 跳过缺失分区时记录的缺失描述。
    missing: Vec<String>,
}

impl<'a> AsyncMergePartitionStats<'a> {
    /// 构造尚未执行的合并任务。
    pub fn new(provider: &'a dyn PartitionStatsProvider, table_id: i64, item_count: usize) -> Self {
        Self {
            provider,
            table_id,
            item_count,
            result: None,
            missing: Vec::new(),
        }
    }

    /// 合并前准备。
    ///
    /// `item_count == 0` 是合法状态：Go 会在未显式传入直方图 ID 时从
    /// 非虚拟列解析 ID，而最终解析结果仍可能为空。此时合并仍需汇总分区行数。
    pub fn prepare(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// 执行合并，写入 `result` / `missing`；`cancelled` 为真时可中途退出。
    pub fn merge(&mut self, options: MergeOptions, cancelled: &AtomicBool) -> Result<(), String> {
        self.result = None;
        self.missing.clear();
        if cancelled.load(Ordering::Acquire) {
            return Err("query interrupted".into());
        }
        self.prepare()?;
        let result = merge_partition_stats_to_global(
            self.provider,
            self.table_id,
            self.item_count,
            options,
            cancelled,
        )?;
        self.missing = result.missing_partition_stats.clone();
        self.result = Some(result);
        Ok(())
    }

    /// 取得合并结果引用。
    pub fn result(&self) -> Option<&GlobalStats> {
        self.result.as_ref()
    }

    /// 取得缺失分区描述列表。
    pub fn missing_partitions(&self) -> &[String] {
        &self.missing
    }
}
