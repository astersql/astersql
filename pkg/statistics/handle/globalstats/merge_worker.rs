// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// TopN 统计合并工作器与相关统计基元类型。
//
// 在把各分区 TopN 合成全局 TopN 时，需要把“只在部分分区的 TopN 中出现、
// 在其他分区仍落在直方图（Histogram）桶内”的值从直方图中踢出并累加频次，
// 避免全局频次低估。`TopNStatsMergeWorker` 按分区下标区间执行该过程，
// 并通过 `cancelled` 支持协作式取消。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Default, PartialEq)]
/// 直方图单个桶：累计行数与桶内近似 NDV（不同值个数）。
pub struct Bucket {
    pub count: i64,
    pub ndv: i64,
}
#[derive(Clone, Debug, Default, PartialEq)]
/// 列/索引直方图：桶序列及可选的精确计数值（用于从桶中踢出 TopN 项）。
pub struct Histogram {
    pub id: i64,
    pub ndv: i64,
    pub buckets: Vec<Bucket>,
    pub exact_counts: HashMap<Vec<u8>, f64>,
}
impl Histogram {
    /// 从精确计数表移除指定编码值，返回其频次（缺失则为 0）。
    pub fn remove_value(&mut self, value: &[u8]) -> f64 {
        self.exact_counts.remove(value).unwrap_or(0.0)
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// TopN 中单个高频值：编码字节与出现次数。
pub struct TopNMeta {
    pub encoded: Vec<u8>,
    pub count: u64,
}
#[derive(Clone, Debug, Default, PartialEq)]
/// 列/索引的 Top-N 高频值集合。
pub struct TopN {
    pub values: Vec<TopNMeta>,
}
impl TopN {
    /// 所有 TopN 项频次之和。
    pub fn total_count(&self) -> u64 {
        self.values.iter().map(|item| item.count).sum()
    }
    /// 判断编码值是否已在本 TopN 集合中。
    pub fn contains(&self, value: &[u8]) -> bool {
        self.values.iter().any(|item| item.encoded == value)
    }
}
#[derive(Clone, Debug, Default)]
/// Count-Min Sketch：近似频次结构，按编码键累加计数。
pub struct CmSketch {
    pub counters: HashMap<Vec<u8>, u64>,
}
impl CmSketch {
    /// 将另一份 CMSketch 的计数合并进自身（同键相加）。
    pub fn merge(&mut self, other: &Self) {
        for (key, value) in &other.counters {
            *self.counters.entry(key.clone()).or_default() += value;
        }
    }
}
#[derive(Clone, Debug, Default)]
/// Flajolet–Martin Sketch：用于估计 NDV；合并时取较大估计值。
pub struct FmSketch {
    pub ndv: i64,
}
impl FmSketch {
    /// 合并另一份 FMSketch：NDV 取两者较大值。
    pub fn merge(&mut self, other: &Self) {
        // 合并 NDV 估计时取 max，避免重复计数导致过高估计。
        self.ndv = self.ndv.max(other.ndv)
    }
}

#[derive(Clone, Debug, Default)]
/// 一次合并作业持有的各分区直方图与 TopN 列表。
pub struct StatsWrapper {
    pub histograms: Vec<Histogram>,
    pub top_ns: Vec<TopN>,
}
#[derive(Clone, Copy, Debug)]
/// 工作器待处理的分区下标半开区间 `[start, end)`。
pub struct TopNMergeTask {
    pub start: usize,
    pub end: usize,
}
#[derive(Clone, Debug, Default)]
/// 合并任务结果；出错时携带可中断查询类错误信息。
pub struct TopNMergeResponse {
    pub error: Option<String>,
}

/// 按任务区间遍历分区 TopN，汇总全局频次并回填直方图踢出项。
pub struct TopNStatsMergeWorker<'a> {
    pub wrapper: &'a mut StatsWrapper,
    pub counter: HashMap<Vec<u8>, f64>,
    pub cancelled: &'a AtomicBool,
}
impl TopNStatsMergeWorker<'_> {
    /// 执行 `[start,end)` 内各分区 TopN 的频次汇总；`version>=2` 时跳过自身分区回填。
    pub fn run_task(&mut self, task: TopNMergeTask, version: i64) -> TopNMergeResponse {
        if task.start > task.end || task.end > self.wrapper.top_ns.len() {
            return TopNMergeResponse {
                error: Some("invalid TopN merge task range".into()),
            };
        }
        for index in task.start..task.end {
            // 协作式取消：查询被 kill 时立即返回中断错误。
            if self.cancelled.load(Ordering::Acquire) {
                return TopNMergeResponse {
                    error: Some("query interrupted".into()),
                };
            }
            let values = self.wrapper.top_ns[index].values.clone();
            for value in values {
                if self.cancelled.load(Ordering::Acquire) {
                    return TopNMergeResponse {
                        error: Some("query interrupted".into()),
                    };
                }
                // 首次见到该编码时，还需从其他分区直方图中补齐未进 TopN 的频次。
                let first = !self.counter.contains_key(&value.encoded);
                *self.counter.entry(value.encoded.clone()).or_default() += value.count as f64;
                if !first {
                    continue;
                }
                for part in 0..self.wrapper.top_ns.len() {
                    if self.cancelled.load(Ordering::Acquire) {
                        return TopNMergeResponse {
                            error: Some("query interrupted".into()),
                        };
                    }
                    // analyze v2：当前分区已在 TopN 计入，跳过；其他分区若已含该值也跳过。
                    if (version >= 2 && part == index)
                        || self.wrapper.top_ns[part].contains(&value.encoded)
                    {
                        continue;
                    }
                    let count = self.wrapper.histograms[part].remove_value(&value.encoded);
                    *self.counter.entry(value.encoded.clone()).or_default() += count;
                }
            }
        }
        TopNMergeResponse::default()
    }
    /// 消费工作器，返回编码值到累计频次的映射。
    pub fn result(self) -> HashMap<Vec<u8>, f64> {
        self.counter
    }
}
