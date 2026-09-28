// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 表采样（Table Sample）执行器。
//
// 对应 Go 的 `TableSampleExecutor` / `TableRegionSampler`：按 Region（键空间分片）
// 随机抽取若干区间，并对每个区间扫描首个 KV，解码后写入结果 chunk。
// 用于统计信息收集等需要代表性样本行的场景。`TableSampleRuntime` 抽象存储与解码边界。
#![allow(non_snake_case)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

/// 半开键区间 `[start_key, end_key)`，对应一个待采样的 Region 片段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyRange {
    /// 区间起始键（含）。
    pub start_key: Vec<u8>,
    /// 区间结束键（不含）。
    pub end_key: Vec<u8>,
}

/// 采样得到的单条 KV：handle（行标识）与原始 value。
pub struct SampleKv<H, V> {
    /// 行 handle / 主键编码。
    pub handle: H,
    /// 行原始编码值。
    pub value: V,
}

/// 表采样运行时边界：请求 chunk、Region 切分、并发扫描与行解码。
pub trait TableSampleRuntime: Clone + Send {
    /// 会话 / 执行上下文。
    type Context;
    /// 输出请求（chunk）类型。
    type Request;
    /// 行 handle 类型。
    type Handle: Send;
    /// 原始 value 类型。
    type Value: Send;
    /// 解码后的行。
    type Row;
    /// 采样列描述。
    type Column;
    /// 列解码映射。
    type DecodeColumnMap;
    /// 错误类型。
    type Error: Send;

    /// 清空输出请求，准备本轮写入。
    fn reset_request(&self, request: &mut Self::Request);
    /// 本轮还需要采样的行数。
    fn required_rows(&self, request: &Self::Request) -> usize;
    /// 请求是否已满（达到 max chunk size）。
    fn request_is_full(&self, request: &Self::Request) -> bool;
    /// 将解码行追加到请求。
    fn append_row(&self, request: &mut Self::Request, row: Self::Row);
    /// 物理表对应的整表键范围。
    fn table_key_range(&self, physical_table_id: i64) -> KeyRange;
    /// 分区表各分区键范围；空表示非分区。
    fn partition_key_ranges(&self) -> Vec<KeyRange>;
    /// 查询范围内的 Region 边界键；`None` 表示不切分。
    fn region_boundaries(&mut self, range: &KeyRange) -> Result<Option<Vec<Vec<u8>>>, Self::Error>;
    /// 表无 Region 时的错误。
    fn no_regions_error(&self) -> Self::Error;
    /// 并发扫描工作线程数上限。
    fn executor_concurrency(&self) -> usize;
    /// 按升序在给定区间扫描第一条 KV。
    fn scan_first_kv(
        &mut self,
        range: &KeyRange,
        start_timestamp: u64,
    ) -> Result<Option<SampleKv<Self::Handle, Self::Value>>, Self::Error>;
    /// 构建采样列与解码映射。
    fn build_sample_columns(
        &mut self,
    ) -> Result<(Vec<Self::Column>, Self::DecodeColumnMap), Self::Error>;
    /// 将 handle/value 解码为输出行。
    fn decode_row(
        &mut self,
        handle: Self::Handle,
        value: Self::Value,
        columns: &[Self::Column],
        column_map: &Self::DecodeColumnMap,
    ) -> Result<Self::Row, Self::Error>;
    /// 重置行级临时映射（每行解码前调用）。
    fn reset_row_map(&mut self);
}

/// 表采样执行器：对外暴露 Open / Next / Close，内部委托 `TableRegionSampler`。
pub struct TableSampleExecutor<R: TableSampleRuntime> {
    /// Region 采样与写 chunk 的核心状态机。
    pub sampler: TableRegionSampler<R>,
}

impl<R: TableSampleRuntime> TableSampleExecutor<R> {
    /// 打开执行器（采样路径无需额外初始化）。
    pub fn Open(&mut self, _context: &mut R::Context) -> Result<(), R::Error> {
        Ok(())
    }

    /// 拉取一批采样行；已耗尽区间时返回空请求。
    pub fn Next(
        &mut self,
        _context: &mut R::Context,
        request: &mut R::Request,
    ) -> Result<(), R::Error> {
        self.sampler.runtime.reset_request(request);
        if self.sampler.finished() {
            return Ok(());
        }
        self.sampler.writeChunk(request)
    }

    /// 关闭执行器。
    pub fn Close(&mut self) -> Result<(), R::Error> {
        Ok(())
    }
}

/// 按 Region 切分表键空间并随机抽样的采样器。
pub struct TableRegionSampler<R: TableSampleRuntime> {
    /// 存储 / 解码运行时。
    pub runtime: R,
    /// 快照起始时间戳（MVCC 读版本）。
    pub start_timestamp: u64,
    /// 物理表 ID。
    pub physical_table_id: i64,
    /// 是否按降序遍历键。
    pub descending: bool,
    /// 懒初始化后的待采样区间池；`None` 表示尚未切分。
    pub ranges: Option<Vec<KeyRange>>,
}

/// 构造尚未切分区间的 `TableRegionSampler`。
pub fn newTableRegionSampler<R: TableSampleRuntime>(
    runtime: R,
    start_timestamp: u64,
    physical_table_id: i64,
    descending: bool,
) -> TableRegionSampler<R> {
    TableRegionSampler {
        runtime,
        start_timestamp,
        physical_table_id,
        descending,
        ranges: None,
    }
}

impl<R: TableSampleRuntime> TableRegionSampler<R> {
    /// 初始化区间、顺序取出，并将采样行写入请求。
    pub fn writeChunk(&mut self, request: &mut R::Request) -> Result<(), R::Error> {
        self.initRanges()?;
        while self.runtime.required_rows(request) > 0 && !self.finished() {
            let ranges = self.pickRanges(self.runtime.required_rows(request));
            self.writeChunkFromRanges(ranges, request)?;
        }
        Ok(())
    }

    /// 懒切分并排序全部待采样区间（只执行一次）。
    pub fn initRanges(&mut self) -> Result<(), R::Error> {
        if self.ranges.is_some() {
            return Ok(());
        }
        let mut ranges = self.splitTableRanges()?;
        sortRanges(&mut ranges, self.descending);
        self.ranges = Some(ranges);
        Ok(())
    }

    /// 按已排序顺序取出至多 `count` 个区间。
    pub fn pickRanges(&mut self, count: usize) -> Vec<KeyRange> {
        let ranges = self.ranges.as_mut().expect("ranges initialized");
        let count = count.min(ranges.len());
        ranges.drain(..count).collect()
    }

    /// 对给定区间并发扫首 KV，解码后追加到请求直至满。
    pub fn writeChunkFromRanges(
        &mut self,
        ranges: Vec<KeyRange>,
        request: &mut R::Request,
    ) -> Result<(), R::Error> {
        let (columns, column_map) = self.runtime.build_sample_columns()?;
        let samples = scanFirstKVForEachRange(self.runtime.clone(), ranges, self.start_timestamp)?;
        for sample in samples {
            if self.runtime.request_is_full(request) {
                break;
            }
            let row =
                self.runtime
                    .decode_row(sample.handle, sample.value, &columns, &column_map)?;
            self.runtime.append_row(request, row);
            self.runtime.reset_row_map();
        }
        Ok(())
    }

    /// 将表（或各分区）键范围按 Region 边界切成多段。
    pub fn splitTableRanges(&mut self) -> Result<Vec<KeyRange>, R::Error> {
        // 分区表用各分区范围，否则用整表范围。
        let partitions = self.runtime.partition_key_ranges();
        let source_ranges = if partitions.is_empty() {
            vec![self.runtime.table_key_range(self.physical_table_id)]
        } else {
            partitions
        };
        let mut ranges = Vec::new();
        for range in source_ranges {
            ranges.extend(splitIntoMultiRanges(&mut self.runtime, range)?);
        }
        Ok(ranges)
    }

    /// 转发运行时的行映射重置。
    pub fn resetRowMap(&mut self) {
        self.runtime.reset_row_map();
    }

    /// 区间池已初始化且为空时表示采样结束。
    pub fn finished(&self) -> bool {
        self.ranges.as_ref().is_some_and(Vec::is_empty)
    }
}

/// 用 Region 边界把一个键范围切成多个不相交子区间。
pub fn splitIntoMultiRanges<R: TableSampleRuntime>(
    runtime: &mut R,
    range: KeyRange,
) -> Result<Vec<KeyRange>, R::Error> {
    let Some(mut boundaries) = runtime.region_boundaries(&range)? else {
        // 后端未提供边界时保持原区间不变。
        return Ok(vec![range]);
    };
    if boundaries.is_empty() {
        return Err(runtime.no_regions_error());
    }
    // 只保留严格落在开区间内的边界，排序去重后切段。
    boundaries.retain(|key| *key > range.start_key && *key < range.end_key);
    boundaries.sort();
    boundaries.dedup();
    let mut start = range.start_key;
    let mut ranges = Vec::with_capacity(boundaries.len() + 1);
    for end in boundaries {
        ranges.push(KeyRange {
            start_key: std::mem::replace(&mut start, end.clone()),
            end_key: end,
        });
    }
    ranges.push(KeyRange {
        start_key: start,
        end_key: range.end_key,
    });
    Ok(ranges)
}

/// 按 start_key 升序排序；`descending` 时再整体反转。
pub fn sortRanges(ranges: &mut [KeyRange], descending: bool) {
    ranges.sort_by(|left, right| left.start_key.cmp(&right.start_key));
    if descending {
        ranges.reverse();
    }
}

/// 并发地对每个区间扫描第一条 KV，汇总为样本列表。
pub fn scanFirstKVForEachRange<R: TableSampleRuntime>(
    runtime: R,
    ranges: Vec<KeyRange>,
    start_timestamp: u64,
) -> Result<Vec<SampleKv<R::Handle, R::Value>>, R::Error> {
    if ranges.is_empty() {
        return Ok(Vec::new());
    }
    // 工作线程数不超过区间数，用原子下标分发任务。
    let concurrency = runtime.executor_concurrency().max(1).min(ranges.len());
    let ranges = Arc::new(ranges);
    let next = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..concurrency {
            let mut worker_runtime = runtime.clone();
            let ranges = Arc::clone(&ranges);
            let next = Arc::clone(&next);
            let sender = sender.clone();
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= ranges.len() {
                        break;
                    }
                    let result = worker_runtime.scan_first_kv(&ranges[index], start_timestamp);
                    if sender.send((index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
    });
    let mut results = receiver.into_iter().collect::<Vec<_>>();
    results.sort_unstable_by_key(|(index, _)| *index);
    let mut samples = Vec::with_capacity(results.len());
    for (_index, result) in results {
        if let Some(sample) = result? {
            samples.push(sample);
        }
    }
    Ok(samples)
}
