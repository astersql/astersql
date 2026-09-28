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

// 多路归并迭代器：合并多个有序 SST 数据文件或范围属性文件。
//
// `MergeKVIter` 用最小堆按 key 归并 KV；`MergePropIter` 按 FirstKey 归并
// RangeProperty（范围属性）。输入须各自有序，输出才保证全局有序。

// 当前不会打开对象存储、启动后台任务或执行业务动作；reader、任务组、通道和指标类型等待跨文件接线。
//
// use std::collections::HashMap;
// use std::sync::atomic::{AtomicBool, Ordering};
// use std::sync::Arc;
//
/// HeapElem 对应 Go 的 heapElem：提供排序键、共享内存转拥有内存和近似元素大小。
// pub trait HeapElem: Clone {
//     fn sortKey(&self) -> &[u8];
//     fn cloneInnerFields(&mut self);
//     fn len(&self) -> usize;
// }
// */
use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use crate::codec::RangeProperty;
use crate::kv_reader::{DefaultReadBufferSize, KVReader};
use crate::stat_reader::StatsReader;
use crate::writer::MultipleFilesStat;
use crate::{Error, MemoryStorage, Result};

/// 归并堆中的键值元素；Key 作为排序键。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KVPair {
    pub Key: Vec<u8>,
    pub Value: Vec<u8>,
}

/// 最小堆条目：绑定排序键、来源 reader 下标与载荷。
#[derive(Clone, Debug, Eq, PartialEq)]
struct HeapEntry<T> {
    key: Vec<u8>,
    reader_index: usize,
    value: T,
}
impl<T: Eq> Ord for HeapEntry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap 是最大堆，反转比较以实现按 key 升序弹出。
        other
            .key
            .cmp(&self.key)
            .then_with(|| other.reader_index.cmp(&self.reader_index))
    }
}
impl<T: Eq> PartialOrd for HeapEntry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 多文件 KV 归并迭代器：每个路径一个 `KVReader`，堆顶为全局最小 key。
pub struct MergeKVIter {
    readers: Vec<Option<KVReader>>,
    paths: Vec<String>,
    heap: BinaryHeap<HeapEntry<KVPair>>,
    current: Option<KVPair>,
    current_reader: Option<usize>,
    error: Option<Error>,
    closed: bool,
    check_hotspot: bool,
    hotspot_counts: Vec<usize>,
    hotspot_period: usize,
    hotspot_since_check: usize,
    current_hotspot: Option<usize>,
}
impl MergeKVIter {
    /// 打开各路径、读取首元素并建堆；空文件对应槽位为 None。
    pub fn new(
        paths: &[String],
        storage: &MemoryStorage,
        offsets: Option<&[u64]>,
        buffer_size: usize,
    ) -> Result<Self> {
        Self::new_with_options(paths, storage, offsets, buffer_size, false, 1)
    }

    /// 带热点检测与外层并发参数的构造入口，对照 Go `NewMergeKVIter`。
    pub fn new_with_options(
        paths: &[String],
        storage: &MemoryStorage,
        offsets: Option<&[u64]>,
        buffer_size: usize,
        check_hotspot: bool,
        outer_concurrency: usize,
    ) -> Result<Self> {
        if outer_concurrency == 0 {
            return Err(Error::InvalidData(
                "outerConcurrency must be positive".into(),
            ));
        }
        if paths.is_empty() {
            return Err(Error::InvalidData("no reader openers".into()));
        }
        if offsets.is_some_and(|v| v.len() != paths.len()) {
            return Err(Error::InvalidData("reader offset count mismatch".into()));
        }
        let mut readers: Vec<Option<KVReader>> = Vec::with_capacity(paths.len());
        let mut heap = BinaryHeap::new();
        for (index, path) in paths.iter().enumerate() {
            let offset = offsets.map_or(0, |v| v[index]);
            let mut reader = match KVReader::from_storage(storage, path, offset, buffer_size.max(1))
            {
                Ok(reader) => reader,
                Err(error) if error.is_eof() => {
                    readers.push(None);
                    continue;
                }
                Err(error) => {
                    for reader in readers.iter_mut().flatten() {
                        let _ = reader.close();
                    }
                    return Err(error);
                }
            };
            match reader.next_kv() {
                Ok((key, value)) => {
                    heap.push(HeapEntry {
                        key: key.clone(),
                        reader_index: index,
                        value: KVPair {
                            Key: key,
                            Value: value,
                        },
                    });
                    readers.push(Some(reader));
                }
                // 空文件：关闭自身，堆中不放哨兵。
                Err(error) if error.is_eof() => readers.push(None),
                Err(error) => {
                    let _ = reader.close();
                    for reader in readers.iter_mut().flatten() {
                        let _ = reader.close();
                    }
                    return Err(error);
                }
            }
        }
        Ok(Self {
            readers,
            paths: paths.to_vec(),
            heap,
            current: None,
            current_reader: None,
            error: None,
            closed: false,
            check_hotspot,
            hotspot_counts: vec![0; paths.len()],
            hotspot_period: 10_000,
            hotspot_since_check: 0,
            current_hotspot: None,
        })
    }
    /// 弹出堆顶并补充同源下一键；返回 false 表示耗尽或已出错。
    pub fn next(&mut self) -> bool {
        if self.closed || self.error.is_some() {
            return false;
        }
        let Some(entry) = self.heap.pop() else {
            self.current = None;
            return false;
        };
        self.current = Some(entry.value);
        self.current_reader = Some(entry.reader_index);
        if self.check_hotspot {
            self.hotspot_counts[entry.reader_index] += 1;
            self.hotspot_since_check += 1;
        }
        let reader = self.readers[entry.reader_index].as_mut().unwrap();
        match reader.next_kv() {
            Ok((key, value)) => self.heap.push(HeapEntry {
                key: key.clone(),
                reader_index: entry.reader_index,
                value: KVPair {
                    Key: key,
                    Value: value,
                },
            }),
            Err(error) if error.is_eof() => {
                let _ = reader.close();
                self.readers[entry.reader_index] = None;
            }
            Err(error) => self.error = Some(error),
        }
        if self.check_hotspot && self.hotspot_since_check >= self.hotspot_period {
            self.rebalance_hotspot();
        }
        true
    }

    /// 每个检测周期只在某一路严格占多数时启用并发预取；热点改变时立即关闭旧热点。
    fn rebalance_hotspot(&mut self) {
        let total = self.hotspot_counts.iter().sum::<usize>();
        let next = self
            .hotspot_counts
            .iter()
            .enumerate()
            .filter(|(index, count)| self.readers[*index].is_some() && **count * 2 > total)
            .map(|(index, _)| index)
            .next();
        if self.current_hotspot != next {
            if let Some(index) = self.current_hotspot
                && let Some(reader) = self.readers[index].as_mut()
            {
                let _ = reader.switch_concurrent_mode(false);
            }
            if let Some(index) = next
                && let Some(reader) = self.readers[index].as_mut()
            {
                if reader
                    .enable_concurrent_read(
                        1,
                        crate::byte_reader::ConcurrentReaderBufferSizePerConc
                            .load(std::sync::atomic::Ordering::Acquire),
                    )
                    .is_ok()
                {
                    let _ = reader.switch_concurrent_mode(true);
                }
            }
            self.current_hotspot = next;
        }
        self.hotspot_counts.fill(0);
        self.hotspot_since_check = 0;
    }
    /// 当前键；未定位时返回空切片。
    pub fn key(&self) -> &[u8] {
        self.current.as_ref().map_or(&[], |v| v.Key.as_slice())
    }
    /// 当前值；未定位时返回空切片。
    pub fn value(&self) -> &[u8] {
        self.current.as_ref().map_or(&[], |v| v.Value.as_slice())
    }
    /// 迭代过程中捕获的非 EOF 错误。
    pub fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }
    /// 关闭所有仍存活的 reader，返回第一个关闭错误。
    pub fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let mut first = None;
        for reader in self.readers.iter_mut().flatten() {
            let _ = reader.switch_concurrent_mode(false);
            if let Err(error) = reader.close() {
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
        self.closed = true;
        if let Some(error) = first {
            Err(error)
        } else {
            Ok(())
        }
    }
    /// 调整热点检测周期；测试使用较小值验证切换顺序。
    pub fn set_hotspot_check_period(&mut self, period: usize) {
        self.hotspot_period = period.max(1);
    }
    /// 返回指定 reader 的（期望并发，当前并发）状态；已关闭 reader 返回 None。
    pub fn reader_concurrent_mode(&self, index: usize) -> Option<(bool, bool)> {
        self.readers
            .get(index)
            .and_then(Option::as_ref)
            .map(KVReader::concurrent_mode)
    }
    /// 当前仍存活的 reader 数量。
    pub fn open_reader_count(&self) -> usize {
        self.readers.iter().flatten().count()
    }
    /// Go 风格别名：`next`。
    pub fn Next(&mut self) -> bool {
        self.next()
    }
    /// Go 风格别名：`key`。
    pub fn Key(&self) -> &[u8] {
        self.key()
    }
    /// Go 风格别名：`value`。
    pub fn Value(&self) -> &[u8] {
        self.value()
    }
    /// Go 风格别名：`error`。
    pub fn Error(&self) -> Option<&Error> {
        self.error()
    }
    /// Go 风格别名：`close`。
    pub fn Close(&mut self) -> Result<()> {
        self.close()
    }
}

/// 带权重的惰性多路归并器；只打开总权重不超过 limit 的连续 reader 窗口。
pub(crate) struct LimitSizeMergeIter<T: Clone + Ord> {
    sources: Vec<VecDeque<Result<T>>>,
    weights: Vec<i64>,
    next_source: usize,
    active: Vec<bool>,
    active_weight: i64,
    limit: i64,
    heap: BinaryHeap<Reverse<(T, usize)>>,
    current: Option<T>,
    error: Option<Error>,
    closed: bool,
}

impl<T: Clone + Ord> LimitSizeMergeIter<T> {
    pub(crate) fn new(sources: Vec<Vec<Result<T>>>, weights: Vec<i64>, limit: i64) -> Result<Self> {
        if sources.is_empty() || sources.len() != weights.len() || limit <= 0 {
            return Err(Error::InvalidData(
                "invalid weighted merge configuration".into(),
            ));
        }
        if weights.iter().any(|weight| *weight <= 0) {
            return Err(Error::InvalidData("reader weights must be positive".into()));
        }
        let mut result = Self {
            active: vec![false; sources.len()],
            sources: sources.into_iter().map(VecDeque::from).collect(),
            weights,
            next_source: 0,
            active_weight: 0,
            limit,
            heap: BinaryHeap::new(),
            current: None,
            error: None,
            closed: false,
        };
        result.open_available()?;
        Ok(result)
    }

    fn open_available(&mut self) -> Result<()> {
        while self.next_source < self.sources.len() {
            let index = self.next_source;
            let weight = self.weights[index];
            if self.active_weight > 0 && self.active_weight + weight > self.limit {
                break;
            }
            self.next_source += 1;
            self.active[index] = true;
            self.active_weight += weight;
            match self.sources[index].pop_front() {
                Some(Ok(value)) => self.heap.push(Reverse((value, index))),
                Some(Err(error)) => {
                    self.close_source(index);
                    return Err(error);
                }
                None => self.close_source(index),
            }
        }
        Ok(())
    }

    fn close_source(&mut self, index: usize) {
        if self.active[index] {
            self.active[index] = false;
            self.active_weight -= self.weights[index];
        }
    }

    pub(crate) fn next(&mut self) -> bool {
        if self.closed || self.error.is_some() {
            return false;
        }
        if self.heap.is_empty()
            && let Err(error) = self.open_available()
        {
            self.error = Some(error);
            return false;
        }
        let Some(Reverse((value, index))) = self.heap.pop() else {
            return false;
        };
        self.current = Some(value);
        match self.sources[index].pop_front() {
            Some(Ok(next)) => self.heap.push(Reverse((next, index))),
            Some(Err(error)) => {
                self.close_source(index);
                self.error = Some(error);
            }
            None => {
                self.close_source(index);
                if let Err(error) = self.open_available() {
                    self.error = Some(error);
                }
            }
        }
        true
    }

    pub(crate) fn current(&self) -> Option<&T> {
        self.current.as_ref()
    }

    pub(crate) fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    pub(crate) fn active_weight(&self) -> i64 {
        self.active_weight
    }

    pub(crate) fn close(&mut self) {
        self.active.fill(false);
        self.active_weight = 0;
        self.heap.clear();
        self.closed = true;
    }
}
/// 对应 Go NewMergeKVIter；缓冲大小至少为 1。
pub fn NewMergeKVIter(
    paths: &[String],
    storage: &MemoryStorage,
    offsets: Option<&[u64]>,
    buffer_size: usize,
) -> Result<MergeKVIter> {
    MergeKVIter::new(
        paths,
        storage,
        offsets,
        buffer_size.max(DefaultReadBufferSize.min(buffer_size.max(1))),
    )
}

/// 属性归并堆条目：按 FirstKey 排序，并记录外层/内层文件下标。
#[derive(Clone, Debug, Eq, PartialEq)]
struct PropertyEntry {
    property: RangeProperty,
    outer: usize,
    inner: usize,
    source: usize,
}
impl Ord for PropertyEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .property
            .FirstKey
            .cmp(&self.property.FirstKey)
            .then_with(|| other.outer.cmp(&self.outer))
            .then_with(|| other.inner.cmp(&self.inner))
    }
}
impl PartialOrd for PropertyEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 多组 MultipleFilesStat 的范围属性归并迭代器。
pub struct MergePropIter {
    heap: BinaryHeap<PropertyEntry>,
    sources: Vec<Vec<RangeProperty>>,
    positions: Vec<usize>,
    current: Option<PropertyEntry>,
    closed_reader_after_next: bool,
    error: Option<Error>,
    closed: bool,
}
impl MergePropIter {
    /// 按 MinKey 排序统计组，加载各属性文件后建立初始最小堆。
    pub fn new(mut stats: Vec<MultipleFilesStat>, storage: &MemoryStorage) -> Result<Self> {
        if stats.is_empty() {
            return Err(Error::InvalidData("no property statistics".into()));
        }
        stats.sort_by(|a, b| a.MinKey.cmp(&b.MinKey));
        let mut sources = Vec::new();
        let mut heap = BinaryHeap::new();
        for (outer, stat) in stats.iter().enumerate() {
            for (inner, files) in stat.Filenames.iter().enumerate() {
                // Filenames[i] = [data_path, stat_path]；属性读自统计文件。
                let mut reader = match StatsReader::from_storage(storage, &files[1], 250 * 1024) {
                    Ok(reader) => reader,
                    Err(error) if error.is_eof() => {
                        sources.push(Vec::new());
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let properties_result = (|| {
                    let mut properties = Vec::new();
                    loop {
                        match reader.next_prop() {
                            Ok(property) => properties.push(property),
                            Err(error) if error.is_eof() => return Ok(properties),
                            Err(error) => return Err(error),
                        }
                    }
                })();
                let close_result = reader.close();
                let properties = properties_result?;
                close_result?;
                let source_index = sources.len();
                if let Some(first) = properties.first() {
                    heap.push(PropertyEntry {
                        property: first.clone(),
                        outer,
                        inner,
                        source: source_index,
                    });
                }
                sources.push(properties);
            }
        }
        // positions 从 1 起：堆中已放入每源首条。
        let positions = vec![1; sources.len()];
        Ok(Self {
            heap,
            sources,
            positions,
            current: None,
            closed_reader_after_next: false,
            error: None,
            closed: false,
        })
    }
    /// 弹出堆顶属性，并将来源的下一条推入堆；耗尽则标记 closed_reader_after_next。
    pub fn next(&mut self) -> bool {
        if self.closed || self.error.is_some() {
            return false;
        }
        self.closed_reader_after_next = false;
        let Some(entry) = self.heap.pop() else {
            self.current = None;
            return false;
        };
        let source = entry.source;
        let position = self.positions[source];
        if let Some(property) = self.sources[source].get(position) {
            self.heap.push(PropertyEntry {
                property: property.clone(),
                outer: entry.outer,
                inner: entry.inner,
                source,
            });
            self.positions[source] += 1;
        } else {
            self.closed_reader_after_next = true;
        }
        self.current = Some(entry);
        true
    }
    /// 当前范围属性。
    pub fn current_property(&self) -> Option<&RangeProperty> {
        self.current.as_ref().map(|v| &v.property)
    }
    /// 当前属性所属的 (外层统计组下标, 组内文件下标)。
    pub fn reader_index(&self) -> Option<(usize, usize)> {
        self.current.as_ref().map(|v| (v.outer, v.inner))
    }
    /// 清空堆并标记已关闭。
    pub fn close(&mut self) -> Result<()> {
        self.closed = true;
        self.heap.clear();
        Ok(())
    }
    /// Go 风格别名：`next`。
    pub fn Next(&mut self) -> bool {
        self.next()
    }
    /// Go 风格别名：`current_property`。
    pub fn CurrProperty(&self) -> Option<&RangeProperty> {
        self.current_property()
    }
    /// Go 风格别名：`reader_index`。
    pub fn ReaderIndex(&self) -> Option<(usize, usize)> {
        self.reader_index()
    }
    /// 最近一次 Next 是否耗尽了某个属性源（对应关闭底层 reader）。
    pub fn GetBaseIterCloseReaderFlag(&self) -> bool {
        self.closed_reader_after_next
    }
    /// 迭代错误（本简化实现通常保持 None）。
    pub fn Error(&self) -> Option<&Error> {
        self.error.as_ref()
    }
    /// Go 风格别名：`close`。
    pub fn Close(&mut self) -> Result<()> {
        self.close()
    }
}
/// 对应 Go NewMergePropIter。
pub fn NewMergePropIter(
    stats: Vec<MultipleFilesStat>,
    storage: &MemoryStorage,
) -> Result<MergePropIter> {
    MergePropIter::new(stats, storage)
}
