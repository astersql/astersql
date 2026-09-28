// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// DistSQL SelectResult：协处理器（coprocessor）部分结果的迭代与聚合。
//
// DistSQL 将查询下推到 TiKV/TiFlash 的 coprocessor；本模块提供：
// - `SelectResult` / `SelectResultIter`：按行或原始字节拉取 partial result；
// - `serialSelectResults`：串行消费多个子结果；
// - `sortedSelectResults`：按 `ByItem` 对多路结果做归并有序输出；
// - `selectResultRuntimeStats`：汇总 cop 耗时、缓存命中与批处理计数。
//

// coprocessor partial result 的 SelectResult 接口、排序/串行聚合读取、chunk 解码、
// intermediate outputs 迭代和 runtime stats 汇总；不会拉取真实 TiKV/TiFlash 响应或上报指标。

// ===== 当前实现：精简版 SelectResult（不依赖真实 TiKV 响应） =====

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::time::Duration;

use crate::{DistSqlError, DistSqlResult, ResponseSource, SelectResponse};

#[derive(Clone, Debug, PartialEq)]
/// 行内标量值，用于排序比较与结果缓冲。
pub enum Scalar {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Bytes(Vec<u8>),
    String(String),
}
impl Scalar {
    /// 按类型比较两个标量；跨类型时退化为 Debug 字符串比较。
    fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Null, Self::Null) => Ordering::Equal,
            (Self::Null, _) => Ordering::Less,
            (_, Self::Null) => Ordering::Greater,
            (Self::Int(a), Self::Int(b)) => a.cmp(b),
            (Self::UInt(a), Self::UInt(b)) => a.cmp(b),
            (Self::Float(a), Self::Float(b)) => a.total_cmp(b),
            (Self::Bytes(a), Self::Bytes(b)) => a.cmp(b),
            (Self::String(a), Self::String(b)) => a.cmp(b),
            (a, b) => format!("{a:?}").cmp(&format!("{b:?}")),
        }
    }
}
/// 一行结果，由若干 Scalar 组成。
pub type Row = Vec<Scalar>;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 排序键：列下标与是否降序。
pub struct ByItem {
    pub column: usize,
    pub descending: bool,
}
/// 协处理器部分结果迭代器：支持原始字节、行批量读取与转换为行迭代器。
pub trait SelectResult: Send {
    fn NextRaw(&mut self) -> DistSqlResult<Option<Vec<u8>>>;
    fn Next(&mut self, rows: &mut Vec<Row>, capacity: usize) -> DistSqlResult<()>;
    fn IntoIter(self: Box<Self>) -> DistSqlResult<Box<dyn SelectResultIter>>;
    fn Close(&mut self) -> DistSqlResult<()>;
    fn concurrency(&self) -> Option<(usize, usize)> {
        None
    }
}
/// 按行迭代 SelectResult，并携带 channel 下标（用于中间结果通道）。
pub trait SelectResultIter: Send {
    fn Next(&mut self) -> DistSqlResult<Option<SelectResultRow>>;
    fn Close(&mut self) -> DistSqlResult<()>;
}
#[derive(Clone, Debug, Default, PartialEq)]
/// SelectResultIter 返回的一行及其所属 channel。
pub struct SelectResultRow {
    pub row: Row,
    pub channel: usize,
}
/// 若实现暴露并发度，则返回 `(concurrency, extra_concurrency)`。
pub fn GetSelectResultConcurrency(result: &dyn SelectResult) -> Option<(usize, usize)> {
    result.concurrency()
}

/// 基于 `ResponseSource` 的通用 SelectResult：缓冲行与原始字节，并累计 runtime stats。
pub struct selectResult<S: ResponseSource> {
    source: Option<S>,
    buffered: VecDeque<Row>,
    raw: VecDeque<Vec<u8>>,
    closed: bool,
    concurrency: usize,
    extra_concurrency: usize,
    pub runtime_stats: selectResultRuntimeStats,
}
impl<S: ResponseSource + 'static> selectResult<S> {
    /// 用响应源与主并发度构造 selectResult。
    pub fn new(source: S, concurrency: usize) -> Self {
        Self {
            source: Some(source),
            buffered: VecDeque::new(),
            raw: VecDeque::new(),
            closed: false,
            concurrency,
            extra_concurrency: 0,
            runtime_stats: selectResultRuntimeStats::default(),
        }
    }
    /// 从 ResponseSource 拉取下一个 SelectResponse 并消费；无更多数据时关闭。
    fn fetchResp(&mut self) -> DistSqlResult<bool> {
        if self.closed {
            return Ok(false);
        }
        let Some(response) = self
            .source
            .as_mut()
            .expect("source exists until close")
            .next_response()?
        else {
            self.Close()?;
            return Ok(false);
        };
        self.consume_response(response)
    }
    /// 将响应中的行写入缓冲，并更新扫描 key / 告警等统计。
    fn consume_response(&mut self, response: SelectResponse) -> DistSqlResult<bool> {
        self.runtime_stats.response_count += 1;
        self.runtime_stats.scanned_keys += response.scanned_keys;
        self.runtime_stats.warning_count += response.warnings.len();
        if let Some(error) = response.error {
            self.Close()?;
            return Err(DistSqlError(error));
        }
        for row in response.rows {
            self.raw.push_back(row.join("\t").into_bytes());
            self.buffered
                .push_back(row.into_iter().map(Scalar::String).collect());
        }
        Ok(true)
    }
}
impl<S: ResponseSource + 'static> SelectResult for selectResult<S> {
    // 优先弹出已缓冲的 raw；缓冲空则继续 fetchResp。
    fn NextRaw(&mut self) -> DistSqlResult<Option<Vec<u8>>> {
        loop {
            if let Some(raw) = self.raw.pop_front() {
                if !self.buffered.is_empty() {
                    self.buffered.pop_front();
                }
                return Ok(Some(raw));
            }
            if !self.fetchResp()? {
                return Ok(None);
            }
        }
    }
    fn Next(&mut self, rows: &mut Vec<Row>, capacity: usize) -> DistSqlResult<()> {
        while rows.len() < capacity {
            if let Some(row) = self.buffered.pop_front() {
                if !self.raw.is_empty() {
                    self.raw.pop_front();
                }
                rows.push(row);
                continue;
            }
            if !self.fetchResp()? {
                break;
            }
        }
        Ok(())
    }
    fn IntoIter(self: Box<Self>) -> DistSqlResult<Box<dyn SelectResultIter>> {
        Ok(Box::new(selectResultIter {
            result: self,
            channel: 0,
        }))
    }
    fn Close(&mut self) -> DistSqlResult<()> {
        if !self.closed {
            self.closed = true;
            self.buffered.clear();
            self.raw.clear();
            if let Some(source) = self.source.as_mut() {
                source.close()?;
            }
        }
        Ok(())
    }
    fn concurrency(&self) -> Option<(usize, usize)> {
        Some((self.concurrency, self.extra_concurrency))
    }
}
impl<S: ResponseSource> Drop for selectResult<S> {
    fn drop(&mut self) {
        if !self.closed {
            if let Some(source) = self.source.as_mut() {
                let _ = source.close();
            }
        }
    }
}

/// 将 selectResult 包装为逐行 SelectResultIter。
struct selectResultIter<S: ResponseSource> {
    result: Box<selectResult<S>>,
    channel: usize,
}
impl<S: ResponseSource + 'static> SelectResultIter for selectResultIter<S> {
    fn Next(&mut self) -> DistSqlResult<Option<SelectResultRow>> {
        let mut rows = Vec::new();
        self.result.Next(&mut rows, 1)?;
        Ok(rows.pop().map(|row| SelectResultRow {
            row,
            channel: self.channel,
        }))
    }
    fn Close(&mut self) -> DistSqlResult<()> {
        self.result.Close()
    }
}

/// 串行拼接多个 SelectResult：前一个耗尽后再读下一个。
pub struct serialSelectResults {
    results: Vec<Box<dyn SelectResult>>,
    current: usize,
}
/// 构造串行聚合的 SelectResult。
pub fn NewSerialSelectResults(results: Vec<Box<dyn SelectResult>>) -> Box<dyn SelectResult> {
    Box::new(serialSelectResults {
        results,
        current: 0,
    })
}
impl SelectResult for serialSelectResults {
    // 当前子结果耗尽后推进到下一个 SelectResult。
    fn NextRaw(&mut self) -> DistSqlResult<Option<Vec<u8>>> {
        while self.current < self.results.len() {
            if let Some(data) = self.results[self.current].NextRaw()? {
                return Ok(Some(data));
            }
            self.current += 1;
        }
        Ok(None)
    }
    fn Next(&mut self, rows: &mut Vec<Row>, capacity: usize) -> DistSqlResult<()> {
        while self.current < self.results.len() && rows.len() < capacity {
            let before = rows.len();
            self.results[self.current].Next(rows, capacity)?;
            if rows.len() == before {
                self.current += 1;
            }
        }
        Ok(())
    }
    fn IntoIter(self: Box<Self>) -> DistSqlResult<Box<dyn SelectResultIter>> {
        Err(DistSqlError("not implemented".into()))
    }
    fn Close(&mut self) -> DistSqlResult<()> {
        let mut last_error = None;
        for result in &mut self.results {
            if let Err(error) = result.Close() {
                last_error = Some(error);
            }
        }
        last_error.map_or(Ok(()), Err)
    }
}

/// 通用适配：把任意 SelectResult 的 `Next` 暴露为 SelectResultIter。
struct StreamIter {
    stream: Box<dyn SelectResult>,
}
impl SelectResultIter for StreamIter {
    fn Next(&mut self) -> DistSqlResult<Option<SelectResultRow>> {
        let mut rows = Vec::new();
        self.stream.Next(&mut rows, 1)?;
        Ok(rows.pop().map(|row| SelectResultRow { row, channel: 0 }))
    }
    fn Close(&mut self) -> DistSqlResult<()> {
        self.stream.Close()
    }
}

/// 多路归并排序的 SelectResult：各输入保持有序，按 ByItem 选出当前最小行。
pub struct sortedSelectResults {
    inputs: Vec<Box<dyn SelectResultIter>>,
    heads: Vec<Option<Row>>,
    order: Vec<ByItem>,
    initialized: bool,
    closed: bool,
}
/// 将多个 SelectResult 转为迭代器后构造有序归并结果（常用于分区表）。
pub fn NewSortedSelectResults(
    results: Vec<Box<dyn SelectResult>>,
    order: Vec<ByItem>,
) -> DistSqlResult<Box<dyn SelectResult>> {
    let mut inputs = Vec::with_capacity(results.len());
    for result in results {
        inputs.push(result.IntoIter()?);
    }
    let heads = vec![None; inputs.len()];
    Ok(Box::new(sortedSelectResults {
        inputs,
        heads,
        order,
        initialized: false,
        closed: false,
    }))
}
impl sortedSelectResults {
    /// 按 ByItem 顺序比较两行；降序时反转比较结果。
    fn compare_rows(order: &[ByItem], left: &Row, right: &Row) -> Ordering {
        for by in order {
            let ordering = match (left.get(by.column), right.get(by.column)) {
                (Some(left), Some(right)) => left.compare(right),
                (None, None) => Ordering::Equal,
                (None, _) => Ordering::Less,
                (_, None) => Ordering::Greater,
            };
            let ordering = if by.descending {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    }
    /// 惰性初始化：为每个输入预取一行作为堆顶候选。
    fn initialize(&mut self) -> DistSqlResult<()> {
        if self.initialized {
            return Ok(());
        }
        for index in 0..self.inputs.len() {
            self.heads[index] = self.inputs[index].Next()?.map(|row| row.row);
        }
        self.initialized = true;
        Ok(())
    }
    /// 选出当前最小行，并从对应输入推进下一候选。
    // 在各路 heads 中按 order 选最小行，并推进该路下一候选。
    fn next_row(&mut self) -> DistSqlResult<Option<Row>> {
        self.initialize()?;
        let Some(index) = self
            .heads
            .iter()
            .enumerate()
            .filter_map(|(index, row)| row.as_ref().map(|row| (index, row)))
            .min_by(|left, right| Self::compare_rows(&self.order, left.1, right.1))
            .map(|entry| entry.0)
        else {
            return Ok(None);
        };
        let row = self.heads[index].take();
        self.heads[index] = self.inputs[index].Next()?.map(|row| row.row);
        Ok(row)
    }
}
impl SelectResult for sortedSelectResults {
    fn NextRaw(&mut self) -> DistSqlResult<Option<Vec<u8>>> {
        Err(DistSqlError(
            "NextRaw is unsupported for sorted select results".into(),
        ))
    }
    fn Next(&mut self, rows: &mut Vec<Row>, capacity: usize) -> DistSqlResult<()> {
        while rows.len() < capacity {
            let Some(row) = self.next_row()? else {
                break;
            };
            rows.push(row);
        }
        Ok(())
    }
    fn IntoIter(self: Box<Self>) -> DistSqlResult<Box<dyn SelectResultIter>> {
        Err(DistSqlError("not implemented".into()))
    }
    fn Close(&mut self) -> DistSqlResult<()> {
        if !self.closed {
            self.closed = true;
            for input in &mut self.inputs {
                input.Close()?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
/// SelectResult 运行时统计：响应数、告警、扫描 key、缓存命中与 store batch。
pub struct selectResultRuntimeStats {
    pub response_count: usize,
    pub warning_count: usize,
    pub scanned_keys: u64,
    pub cop_response_time: Duration,
    pub cop_cache_hit_num: u64,
    pub store_batched_num: u64,
    pub store_batched_fallback_num: u64,
}
impl selectResultRuntimeStats {
    /// 合并单次 cop 响应的耗时、缓存命中与 store batch 计数。
    pub fn mergeCopRuntimeStats(
        &mut self,
        response_time: Duration,
        cache_hit: bool,
        store_batched: u64,
        fallback: u64,
    ) {
        self.cop_response_time += response_time;
        self.cop_cache_hit_num += u64::from(cache_hit);
        self.store_batched_num += store_batched;
        self.store_batched_fallback_num += fallback;
    }
    /// 累加另一份 runtime stats。
    pub fn Merge(&mut self, other: &Self) {
        self.response_count += other.response_count;
        self.warning_count += other.warning_count;
        self.scanned_keys += other.scanned_keys;
        self.cop_response_time += other.cop_response_time;
        self.cop_cache_hit_num += other.cop_cache_hit_num;
        self.store_batched_num += other.store_batched_num;
        self.store_batched_fallback_num += other.store_batched_fallback_num;
    }
    /// 计算 copr cache 命中率；store batch 计入总任务数。
    pub fn calcCacheHit(&self) -> f64 {
        let total = self.response_count as u64 + self.store_batched_num;
        if total == 0 {
            0.0
        } else {
            self.cop_cache_hit_num as f64 / total as f64
        }
    }
}
impl fmt::Display for selectResultRuntimeStats {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cop_task: {}, response_time: {:?}",
            self.response_count, self.cop_response_time
        )?;
        if astersql_config::get_global_config()
            .tikv_client
            .copr_cache
            .capacity_mb
            > 0
        {
            write!(formatter, ", cache_hit_ratio: {:.2}", self.calcCacheHit())
        } else {
            write!(formatter, ", copr_cache: disabled")
        }
    }
}

/// 为尚未记录 execution summary 的 TiFlash/MPP plan id 填充占位列表。
/// MPP：Massively Parallel Processing，TiFlash 侧并行执行模型。
pub fn FillDummySummariesForTiFlashTasks(
    all_plan_ids: &[i32],
    recorded: &HashMap<i32, usize>,
) -> Vec<i32> {
    all_plan_ids
        .iter()
        .copied()
        .filter(|id| !recorded.contains_key(id))
        .collect()
}
