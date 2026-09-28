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

/*

/// SortedReader 对应 Go 的 sortedReader，统一 KV reader、属性 reader 和二级合并 reader。
pub trait SortedReader<T: HeapElem> {
    fn path(&self) -> &str;
    fn next(&mut self) -> Result<T, Error>;
    /// false -> true 延迟到普通缓存耗尽后扩大预取；true -> false 必须立即释放大缓冲，防止 OOM。
    fn switchConcurrentMode(&mut self, useConcurrent: bool) -> Result<(), Error>;
    fn close(&mut self) -> Result<(), Error>;
}

/// MergeHeapElem 把元素和来源 reader 下标绑定，弹出后才能从正确 reader 补充下一项。
#[derive(Clone)]
struct MergeHeapElem<T: HeapElem> {
    elem: T,
    readerIdx: usize,
}

/// MergeHeap 对应 Go mergeHeap；直接维护最小堆接口，排序依据仅为元素 sortKey。
struct MergeHeap<T: HeapElem>(Vec<MergeHeapElem<T>>);

impl<T: HeapElem> MergeHeap<T> {
    fn Len(&self) -> usize {
        self.0.len()
    }

    fn Less(&self, i: usize, j: usize) -> bool {
        self.0[i].elem.sortKey() < self.0[j].elem.sortKey()
    }

    fn Swap(&mut self, i: usize, j: usize) {
        self.0.swap(i, j);
    }

    fn Push(&mut self, value: MergeHeapElem<T>) {
        self.0.push(value);
        self.0.sort_by(|a, b| b.elem.sortKey().cmp(a.elem.sortKey()));
    }

    fn Pop(&mut self) -> MergeHeapElem<T> {
        // 与 container/heap.Pop 相同，调用方只在 Len > 0 时弹出最小元素。
        self.0.pop().expect("non-empty merge heap")
    }
}

/// ReaderOpenerFn 对应 Go readerOpenerFn；每个闭包只负责打开一个确定路径的 reader。
type ReaderOpenerFn<R> = Box<dyn FnMut() -> Result<R, Error>>;

/// MergeIter 是通用多路归并核心，并记录热点 reader 的访问频率以动态切换并发预取。
struct MergeIter<T: HeapElem, R: SortedReader<T>> {
    h: MergeHeap<T>,
    readers: Vec<Option<R>>,
    curr: Option<T>,
    lastReaderIdx: Option<usize>,
    err: Option<Error>,
    checkHotspot: bool,
    hotspotMap: HashMap<usize, usize>,
    checkHotspotCnt: usize,
    checkHotspotPeriod: usize,
    lastHotspotIdx: Option<usize>,
    elemFromHotspot: Option<T>,
    logger: Logger,
}

/// openAndGetFirstElem 并行打开 readers，随后顺序读取各自首元素。
/// 打开或首读失败会关闭所有已成功打开的 reader；空文件则关闭自身并在结果中保留 None 槽位。
fn openAndGetFirstElem<T: HeapElem, R: SortedReader<T>>(
    openers: &mut [ReaderOpenerFn<R>],
) -> Result<(Vec<Option<R>>, Vec<Option<T>>), Error> {
    // parallelOpenIndexed 对应 errgroup：结果按输入下标写回，完成先后不会改变 reader 顺序。
    let mut readers = parallelOpenIndexed(openers)?;
    let mut elements = vec![None; readers.len()];

    for index in 0..readers.len() {
        let Some(reader) = readers[index].as_mut() else { continue };
        match reader.next() {
            Ok(elem) => elements[index] = Some(elem),
            Err(err) if err.is_eof() => {
                let _ = reader.close();
                readers[index] = None;
            }
            Err(err) => {
                // Go 的 closeReaders 忽略收尾错误，优先返回导致构造失败的原始错误。
                for opened in readers.iter_mut().flatten() {
                    let _ = opened.close();
                }
                return Err(err);
            }
        }
    }
    Ok((readers, elements))
}

/// newMergeIter 打开所有输入并建立初始最小堆；readers 始终保持与 openers 相同的下标顺序。
fn newMergeIter<T: HeapElem, R: SortedReader<T>>(
    ctx: &Context,
    mut readerOpeners: Vec<ReaderOpenerFn<R>>,
    checkHotspot: bool,
) -> Result<MergeIter<T, R>, Error> {
    if readerOpeners.is_empty() {
        return Err(Error::message("no reader openers"));
    }
    let (readers, firstElements) = openAndGetFirstElem(&mut readerOpeners)?;
    let mut heap = MergeHeap(Vec::with_capacity(readers.len()));
    let mut sampleKeySize = 0;
    let mut sampleKeyCnt = 0;

    for (readerIdx, elem) in firstElements.into_iter().enumerate() {
        let Some(elem) = elem else { continue };
        sampleKeySize += elem.len();
        sampleKeyCnt += 1;
        heap.Push(MergeHeapElem { elem, readerIdx });
    }

    // 约每读取 32 MiB 检查一次热点；小元素场景至少间隔 1000 次，空输入使用 10000。
    let checkHotspotPeriod = if sampleKeySize == 0 || sampleKeySize / sampleKeyCnt == 0 {
        10_000
    } else {
        1_000.max((32 * MB) / (sampleKeySize / sampleKeyCnt))
    };

    Ok(MergeIter {
        h: heap,
        readers,
        curr: None,
        lastReaderIdx: None,
        err: None,
        checkHotspot,
        hotspotMap: HashMap::new(),
        checkHotspotCnt: 0,
        checkHotspotPeriod,
        lastHotspotIdx: None,
        elemFromHotspot: None,
        logger: logger(ctx),
    })
}

impl<T: HeapElem, R: SortedReader<T>> MergeIter<T, R> {
    /// close 关闭所有仍存活的 reader，记录每个错误但返回第一个，确保资源收尾不会半途停止。
    fn close(&mut self) -> Result<(), Error> {
        let mut firstErr = None;
        for slot in &mut self.readers {
            let Some(mut reader) = slot.take() else { continue };
            if let Err(err) = reader.close() {
                self.logger.WarnClose(reader.path(), &err);
                if firstErr.is_none() {
                    firstErr = Some(err);
                }
            }
        }
        firstErr.map_or(Ok(()), Err)
    }

    /// next 推进归并迭代器，返回本轮关闭的 reader 下标和是否产出元素。
    /// 正常耗尽时 err 保持 None；其它错误写入 err，之后迭代器不可再用。
    fn next(&mut self) -> (Option<usize>, bool) {
        let mut closedReaderIdx = None;
        if let Some(lastIdx) = self.lastReaderIdx {
            if self.checkHotspot {
                *self.hotspotMap.entry(lastIdx).or_default() += 1;
                self.checkHotspotCnt += 1;
                if self.checkHotspotCnt == self.checkHotspotPeriod {
                    let oldHotspotIdx = self.lastHotspotIdx;
                    self.lastHotspotIdx = self
                        .hotspotMap
                        .iter()
                        .find_map(|(&idx, &count)| (count > self.checkHotspotPeriod / 2).then_some(idx));

                    if oldHotspotIdx != self.lastHotspotIdx {
                        // 关闭旧热点会释放共享大缓冲，必须先复制仍由 curr 借用的 key/value。
                        if let Some(elem) = self.elemFromHotspot.as_mut() {
                            elem.cloneInnerFields();
                        }
                        self.elemFromHotspot = None;
                    }

                    for (idx, reader) in self.readers.iter_mut().enumerate() {
                        let Some(reader) = reader.as_mut() else { continue };
                        if let Err(err) = reader.switchConcurrentMode(self.lastHotspotIdx == Some(idx)) {
                            self.err = Some(err);
                            return (closedReaderIdx, false);
                        }
                    }
                    self.checkHotspotCnt = 0;
                    self.hotspotMap.clear();
                }
            }

            let reader = self.readers[lastIdx].as_mut().expect("last reader remains open");
            match reader.next() {
                Ok(elem) => {
                    if self.checkHotspot && self.lastHotspotIdx == Some(lastIdx) {
                        self.elemFromHotspot = Some(elem.clone());
                    }
                    self.h.Push(MergeHeapElem { elem, readerIdx: lastIdx });
                }
                Err(err) if err.is_eof() => {
                    if let Err(closeErr) = reader.close() {
                        self.logger.WarnClose(reader.path(), &closeErr);
                    }
                    self.readers[lastIdx] = None;
                    self.hotspotMap.remove(&lastIdx);
                    closedReaderIdx = Some(lastIdx);
                }
                Err(err) => {
                    self.logger.ErrorRead(reader.path(), &err);
                    self.err = Some(err);
                    return (closedReaderIdx, false);
                }
            }
        }
        self.lastReaderIdx = None;

        if self.h.Len() == 0 {
            return (closedReaderIdx, false);
        }
        let current = self.h.Pop();
        self.curr = Some(current.elem);
        self.lastReaderIdx = Some(current.readerIdx);
        (closedReaderIdx, true)
    }
}

/// LimitSizeMergeIter 在 MergeIter 之上增加 reader 权重预算，只在旧 reader 关闭后尝试打开更多输入。
struct LimitSizeMergeIter<T: HeapElem, R: SortedReader<T>> {
    mergeIter: MergeIter<T, R>,
    readerOpeners: Vec<ReaderOpenerFn<R>>,
    weights: Vec<i64>,
    nextReaderIdx: usize,
    weightSum: i64,
    limit: i64,
}

/// newLimitSizeMergeIter 打开不超过 limit 的前缀 reader；空文件释放的权重稍后用于继续填充。
fn newLimitSizeMergeIter<T: HeapElem, R: SortedReader<T>>(
    ctx: &Context,
    mut readerOpeners: Vec<ReaderOpenerFn<R>>,
    weights: Vec<i64>,
    limit: i64,
) -> Result<LimitSizeMergeIter<T, R>, Error> {
    if limit <= 0 {
        return Err(Error::message(format!("limit must be positive, got {limit}")));
    }
    let mut end = 0;
    let mut current = 0;
    while end < weights.len() && current + weights[end] <= limit {
        current += weights[end];
        end += 1;
    }

    let initial = readerOpeners.drain(..end).collect();
    let mergeIter = newMergeIter(ctx, initial, false)?;
    let mut result = LimitSizeMergeIter {
        mergeIter,
        readerOpeners,
        weights,
        nextReaderIdx: end,
        weightSum: current,
        limit,
    };
    for idx in 0..result.mergeIter.readers.len() {
        if result.mergeIter.readers[idx].is_none() {
            result.weightSum -= result.weights[idx];
        }
    }
    result.tryOpenMoreReaders()?;
    Ok(result)
}

impl<T: HeapElem, R: SortedReader<T>> LimitSizeMergeIter<T, R> {
    /// tryOpenMoreReaders 按原输入顺序补 reader；顺序是维持全局有序所需的调用方前置条件。
    fn tryOpenMoreReaders(&mut self) -> Result<(), Error> {
        while self.nextReaderIdx < self.weights.len() {
            let weight = self.weights[self.nextReaderIdx];
            if self.weightSum + weight > self.limit {
                return Ok(());
            }
            let mut opener = self.readerOpeners.remove(0);
            self.nextReaderIdx += 1;
            let (mut readers, mut firstElements) = openAndGetFirstElem(std::slice::from_mut(&mut opener))?;
            let reader = readers.remove(0);
            let readerIdx = self.mergeIter.readers.len();
            self.mergeIter.readers.push(reader);
            let Some(elem) = firstElements.remove(0) else { continue };
            self.mergeIter.h.Push(MergeHeapElem { elem, readerIdx });
            self.weightSum += weight;
        }
        Ok(())
    }

    /// next 在 reader 关闭时释放其权重并补充新 reader；若底层堆刚耗尽，要额外推进一次设置 curr。
    fn next(&mut self) -> (bool, Option<usize>) {
        let (closedReaderIdx, mut ok) = self.mergeIter.next();
        let Some(closedIdx) = closedReaderIdx else { return (ok, None) };
        let drained = !ok && self.mergeIter.h.Len() == 0;
        self.weightSum -= self.weights[closedIdx];
        if let Err(err) = self.tryOpenMoreReaders() {
            self.mergeIter.err = Some(err);
            return (false, Some(closedIdx));
        }
        if drained && self.mergeIter.h.Len() > 0 {
            (_, ok) = self.mergeIter.next();
        }
        (ok, Some(closedIdx))
    }
}

/// KVPair 对应 Go 的键值元素；key 和 value 可能暂时借用 ByteReader 的复用缓冲。
#[derive(Clone, Default)]
pub struct KVPair {
    pub Key: Vec<u8>,
    pub Value: Vec<u8>,
}

impl HeapElem for KVPair {
    fn sortKey(&self) -> &[u8] { &self.Key }
    fn cloneInnerFields(&mut self) {
        self.Key = self.Key.clone();
        self.Value = self.Value.clone();
    }
    fn len(&self) -> usize { self.Key.len() + self.Value.len() }
}

/// KVReaderProxy 为 KVReader 补齐 SortedReader 所需的路径与模式切换接口。
struct KVReaderProxy {
    p: String,
    r: KVReader,
}

impl SortedReader<KVPair> for KVReaderProxy {
    fn path(&self) -> &str { &self.p }
    fn next(&mut self) -> Result<KVPair, Error> {
        let (key, value) = self.r.NextKV()?;
        Ok(KVPair { Key: key.to_vec(), Value: value.to_vec() })
    }
    fn switchConcurrentMode(&mut self, useConcurrent: bool) -> Result<(), Error> {
        self.r.byteReader.switchConcurrentMode(useConcurrent)
    }
    fn close(&mut self) -> Result<(), Error> { self.r.Close() }
}

/// MergeKVIter 对外提供多文件 KV 合并迭代，并持有热点并发读取共享的内存池。
pub struct MergeKVIter {
    iter: MergeIter<KVPair, KVReaderProxy>,
    memPool: Pool,
}

/// NewMergeKVIter 为每个路径建立 opener；实际打开由 newMergeIter 并行完成。
pub fn NewMergeKVIter(
    ctx: &Context,
    paths: &[String],
    pathsStartOffset: &[u64],
    exStorage: &Storage,
    readBufferSize: usize,
    checkHotspot: bool,
    outerConcurrency: usize,
) -> Result<MergeKVIter, Error> {
    if outerConcurrency == 0 {
        return Err(Error::message("outerConcurrency must be positive, caller must ensure that the correct value is passed in"));
    }
    let concurrentReaderConcurrency = (concurrentReaderTotalConcurrency / outerConcurrency).max(8);
    let largeBufSize = unsafe { ConcurrentReaderBufferSizePerConc } * concurrentReaderConcurrency;
    // 当前只允许一个 reader 成为热点，所以 pool 只需一个 largeBufSize 大块。
    let memPool = Pool::new(1, largeBufSize);
    let mut openers: Vec<ReaderOpenerFn<KVReaderProxy>> = Vec::with_capacity(paths.len());

    for index in 0..paths.len() {
        let path = paths[index].clone();
        let offset = pathsStartOffset[index];
        let ctx = ctx.clone();
        let storage = exStorage.clone();
        let pool = memPool.clone();
        openers.push(Box::new(move || {
            let mut reader = KVReader::NewKVReader(&ctx, &path, &storage, offset, readBufferSize)?;
            reader.byteReader.mergeSortReadCounter = Some(MergeSortReadBytes.clone());
            reader.byteReader.enableConcurrentRead(
                &storage,
                &path,
                concurrentReaderConcurrency,
                unsafe { ConcurrentReaderBufferSizePerConc },
                &mut pool.NewBuffer(),
            );
            Ok(KVReaderProxy { p: path.clone(), r: reader })
        }));
    }
    let iter = newMergeIter(ctx, openers, checkHotspot)?;
    Ok(MergeKVIter { iter, memPool })
}

impl MergeKVIter {
    pub fn Error(&self) -> Option<&Error> { self.iter.err.as_ref() }
    /// Next 推进到下一项；返回 false 后只能读取 Error 或调用 Close。
    pub fn Next(&mut self) -> bool { self.iter.next().1 }
    pub fn Key(&self) -> &[u8] { &self.iter.curr.as_ref().expect("Next succeeded").Key }
    pub fn Value(&self) -> &[u8] { &self.iter.curr.as_ref().expect("Next succeeded").Value }
    pub fn Close(&mut self) -> Result<(), Error> {
        self.iter.close()?;
        // 必须先关闭各 reader 的子 Buffer，再销毁父 Pool。
        self.memPool.Destroy();
        Ok(())
    }
}

impl HeapElem for RangeProperty {
    fn sortKey(&self) -> &[u8] { &self.FirstKey }
    fn cloneInnerFields(&mut self) {
        self.FirstKey = self.FirstKey.clone();
        self.LastKey = self.LastKey.clone();
    }
    /// 24 是 Offset、Size、Keys 三个 uint64 的固定长度。
    fn len(&self) -> usize { self.FirstKey.len() + self.LastKey.len() + 24 }
}

/// StatReaderProxy 把单个属性文件适配为 SortedReader；属性读取不启用热点并发模式。
struct StatReaderProxy {
    p: String,
    r: StatsReader,
}

impl SortedReader<RangeProperty> for StatReaderProxy {
    fn path(&self) -> &str { &self.p }
    fn next(&mut self) -> Result<RangeProperty, Error> { self.r.NextProp() }
    fn switchConcurrentMode(&mut self, _: bool) -> Result<(), Error> { Ok(()) }
    fn close(&mut self) -> Result<(), Error> { self.r.Close() }
}

/// MergePropBaseIter 合并一个 MultipleFilesStat 内的属性文件，同时按最大重叠数限制打开连接数。
struct MergePropBaseIter {
    iter: LimitSizeMergeIter<RangeProperty, StatReaderProxy>,
    closeReaderFlag: Option<Arc<AtomicBool>>,
    closeToken: CloseToken,
    workers: WaitGroup,
}

/// ReaderAndError 对应预打开任务的单项结果，错误会在 opener 真正消费时传播。
struct ReaderAndError {
    r: Option<StatReaderProxy>,
    err: Option<Error>,
}

const errMergePropBaseIterClosed: &str = "mergePropBaseIter is closed";

/// newMergePropBaseIter 预打开约两倍 limit 的属性 reader，降低权重槽位释放后的阻塞时间。
fn newMergePropBaseIter(
    ctx: &Context,
    multiStat: &MultipleFilesStat,
    exStorage: &Storage,
) -> Result<MergePropBaseIter, Error> {
    let mut limit = if multiStat.MaxOverlappingNum <= 0 {
        multiStat.Filenames.len() as i64
    } else {
        // 多开一个 reader，避免权重刚满时尚未插入下一文件的最小元素而破坏排序。
        multiStat.MaxOverlappingNum + 1
    };
    limit = limit.min(multiStat.Filenames.len() as i64);
    let preOpenLimit = (limit * 2).min(multiStat.Filenames.len() as i64);
    let closeToken = CloseToken::new();
    let workers = WaitGroup::new();

    // PreOpenQueue 内部对应 Go 的嵌套通道：队列有界，单项任务各自保存 reader 或错误。
    let queue = PreOpenQueue::<ReaderAndError>::new((preOpenLimit - limit) as usize);
    for index in limit as usize..multiStat.Filenames.len() {
        let path = multiStat.Filenames[index][1].clone();
        let ctx = ctx.clone();
        let storage = exStorage.clone();
        let token = closeToken.clone();
        let queue = queue.clone();
        workers.Go(move || {
            let result = StatsReader::NewStatsReader(&ctx, &storage, &path, 250 * 1024)
                .map(|r| StatReaderProxy { p: path, r });
            if token.is_closed() {
                if let Ok(mut reader) = result { let _ = reader.close(); }
                return;
            }
            queue.send(match result {
                Ok(reader) => ReaderAndError { r: Some(reader), err: None },
                Err(err) => ReaderAndError { r: None, err: Some(err) },
            });
        });
    }

    let mut openers: Vec<ReaderOpenerFn<StatReaderProxy>> = Vec::with_capacity(multiStat.Filenames.len());
    for index in 0..limit as usize {
        let path = multiStat.Filenames[index][1].clone();
        let ctx = ctx.clone();
        let storage = exStorage.clone();
        openers.push(Box::new(move || {
            let reader = StatsReader::NewStatsReader(&ctx, &storage, &path, 250 * 1024)?;
            Ok(StatReaderProxy { p: path.clone(), r: reader })
        }));
    }
    for _ in limit as usize..multiStat.Filenames.len() {
        let token = closeToken.clone();
        let queue = queue.clone();
        openers.push(Box::new(move || {
            if token.is_closed() {
                return Err(Error::message(errMergePropBaseIterClosed));
            }
            let result = queue.recv().ok_or_else(|| Error::message(errMergePropBaseIterClosed))?;
            if let Some(err) = result.err { return Err(err) }
            result.r.ok_or_else(|| Error::message(errMergePropBaseIterClosed))
        }));
    }

    let weights = vec![1; openers.len()];
    let iter = newLimitSizeMergeIter(ctx, openers, weights, limit)?;
    Ok(MergePropBaseIter { iter, closeReaderFlag: None, closeToken, workers })
}

impl SortedReader<RangeProperty> for MergePropBaseIter {
    fn path(&self) -> &str { "mergePropBaseIter" }
    fn next(&mut self) -> Result<RangeProperty, Error> {
        let (ok, closedReaderIdx) = self.iter.next();
        if closedReaderIdx.is_some() {
            if let Some(flag) = &self.closeReaderFlag { flag.store(true, Ordering::Release) }
        }
        if !ok {
            return Err(self.iter.mergeIter.err.take().unwrap_or_else(Error::eof));
        }
        Ok(self.iter.mergeIter.curr.as_ref().expect("next succeeded").clone())
    }
    fn switchConcurrentMode(&mut self, _: bool) -> Result<(), Error> { Ok(()) }
    /// close 不可与 next 并发：先通知并等待预打开任务，再关闭已经交给归并器的 readers。
    fn close(&mut self) -> Result<(), Error> {
        self.closeToken.close();
        self.workers.Wait();
        self.iter.mergeIter.close()
    }
}

/// MergePropIter 用两级限权归并合并多个 MultipleFilesStat，并暴露底层 reader 关闭事件。
pub struct MergePropIter {
    iter: LimitSizeMergeIter<RangeProperty, MergePropBaseIter>,
    baseCloseReaderFlag: Arc<AtomicBool>,
}

/// NewMergePropIter 先按 MinKey 排序组，再以 MaxOverlappingNum 作为每个二级 reader 的权重。
pub fn NewMergePropIter(
    ctx: &Context,
    mut multiStat: Vec<MultipleFilesStat>,
    exStorage: &Storage,
) -> Result<MergePropIter, Error> {
    multiStat.sort_by(|a, b| a.MinKey.cmp(&b.MinKey));
    let closeReaderFlag = Arc::new(AtomicBool::new(false));
    let weights = multiStat.iter().map(|item| item.MaxOverlappingNum).collect::<Vec<_>>();
    let mut openers: Vec<ReaderOpenerFn<MergePropBaseIter>> = Vec::with_capacity(multiStat.len());

    for stat in multiStat {
        let ctx = ctx.clone();
        let storage = exStorage.clone();
        let flag = closeReaderFlag.clone();
        openers.push(Box::new(move || {
            let mut base = newMergePropBaseIter(&ctx, &stat, &storage)?;
            base.closeReaderFlag = Some(flag.clone());
            Ok(base)
        }));
    }

    // 二级 iterator 也多留一倍阈值空间，原因与 base iterator 的提前打开相同。
    let limit = maxMergeSortOverlapThreshold * 2;
    let iter = newLimitSizeMergeIter(ctx, openers, weights, limit)?;
    Ok(MergePropIter { iter, baseCloseReaderFlag: closeReaderFlag })
}

impl MergePropIter {
    pub fn Error(&self) -> Option<&Error> { self.iter.mergeIter.err.as_ref() }
    pub fn Next(&mut self) -> bool {
        self.baseCloseReaderFlag.store(false, Ordering::Release);
        self.iter.next().0
    }
    /// GetBaseIterCloseReaderFlag 表示最近一次 Next 是否关闭了任一底层属性 reader。
    pub fn GetBaseIterCloseReaderFlag(&self) -> bool {
        self.baseCloseReaderFlag.load(Ordering::Acquire)
    }
    pub fn CurrProperty(&self) -> &RangeProperty {
        self.iter.mergeIter.curr.as_ref().expect("Next succeeded")
    }
    /// ReaderIndex 返回最近访问的二级 reader 和其内部最近访问的文件 reader 下标。
    pub fn ReaderIndex(&self) -> (usize, usize) {
        let outer = self.iter.mergeIter.lastReaderIdx.expect("iterator positioned");
        let inner = self.iter.mergeIter.readers[outer]
            .as_ref().expect("base reader open")
            .iter.mergeIter.lastReaderIdx.expect("base iterator positioned");
        (outer, inner)
    }
    pub fn Close(&mut self) -> Result<(), Error> { self.iter.mergeIter.close() }
}
*/
