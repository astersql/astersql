// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 多路归并（multi-way merge）：将多个已排序分区流合并为全局有序输出。
//
// 使用最小堆（min-heap）维护各分区当前头部行；弹出最小行后从同一分区取下一行再入堆。
// 数据源可为内存队列、磁盘 run（DiskRun）或尚未完全排序的 SortPartition。

use crate::sort_partition::SortPartition;
use crate::sort_util::{DiskRun, Result, Row, RowComparator, SortError};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// 多路归并的行数据源：按分区编号依次取已排序行。
pub trait multiWayMergeSource {
    /// 初始化数据源（例如对 SortPartition 先完成分区内排序）。
    fn init(&mut self) -> Result<()>;
    /// 从指定分区取出下一行；分区耗尽时返回 None。
    fn next(&mut self, partition_id: usize) -> Result<Option<Row>>;
    /// 分区总数。
    fn getPartitionNum(&self) -> usize;
}

/// 内存中的多路数据源：每个分区是一个行队列。
pub struct memorySource {
    /// 各分区的待归并行队列。
    partitions: Vec<VecDeque<Row>>,
}
impl memorySource {
    /// 由各分区行向量构造内存数据源。
    pub fn new(parts: Vec<Vec<Row>>) -> Self {
        Self {
            partitions: parts.into_iter().map(VecDeque::from).collect(),
        }
    }
}
impl multiWayMergeSource for memorySource {
    fn init(&mut self) -> Result<()> {
        Ok(())
    }
    fn next(&mut self, id: usize) -> Result<Option<Row>> {
        self.partitions
            .get_mut(id)
            .map(VecDeque::pop_front)
            .ok_or_else(|| SortError(format!("partition {id} is out of range")))
    }
    fn getPartitionNum(&self) -> usize {
        self.partitions.len()
    }
}

/// 磁盘 run 多路数据源：将 DiskRun 物化为行队列后归并。
pub struct diskSource {
    /// 各磁盘分区对应的行队列。
    partitions: Vec<VecDeque<Row>>,
}
impl diskSource {
    /// 由多个 DiskRun 构造磁盘数据源。
    pub fn new(parts: Vec<DiskRun>) -> Self {
        Self {
            partitions: parts
                .into_iter()
                .map(|r| VecDeque::from(r.into_rows()))
                .collect(),
        }
    }
}
impl multiWayMergeSource for diskSource {
    fn init(&mut self) -> Result<()> {
        Ok(())
    }
    fn next(&mut self, id: usize) -> Result<Option<Row>> {
        self.partitions
            .get_mut(id)
            .map(VecDeque::pop_front)
            .ok_or_else(|| SortError(format!("disk partition {id} is out of range")))
    }
    fn getPartitionNum(&self) -> usize {
        self.partitions.len()
    }
}

/// SortPartition 多路数据源：init 时对各分区加锁排序，next 取已排序行。
pub struct sortPartitionSource {
    /// 共享的排序分区列表。
    partitions: Vec<Arc<Mutex<SortPartition>>>,
}
impl sortPartitionSource {
    /// 由 SortPartition 列表构造数据源。
    pub fn new(parts: Vec<Arc<Mutex<SortPartition>>>) -> Self {
        Self { partitions: parts }
    }
}
impl multiWayMergeSource for sortPartitionSource {
    fn init(&mut self) -> Result<()> {
        // 归并前先完成各分区内部排序。
        for p in &self.partitions {
            p.lock()
                .map_err(|_| SortError("sort partition lock poisoned".into()))?
                .sort()?;
        }
        Ok(())
    }
    fn next(&mut self, id: usize) -> Result<Option<Row>> {
        self.partitions
            .get(id)
            .ok_or_else(|| SortError(format!("partition {id} is out of range")))?
            .lock()
            .map_err(|_| SortError("sort partition lock poisoned".into()))?
            .getNextSortedRow()
    }
    fn getPartitionNum(&self) -> usize {
        self.partitions.len()
    }
}

/// 堆中元素：一行及其所属分区编号（弹出后从同分区补下一行）。
struct HeapRow {
    row: Row,
    partition: usize,
}

/// 多路归并器：以比较器驱动最小堆，产出全局有序行流。
pub struct multiWayMerger<S: multiWayMergeSource> {
    /// 底层多路数据源。
    source: S,
    /// 最小堆：堆顶为当前全局最小行。
    heap: Vec<HeapRow>,
    /// 行比较器（由排序键定义序）。
    compare: RowComparator,
    /// 是否已完成 init（从各分区取首行建堆）。
    initialized: bool,
}

/// 构造多路归并器，尚未从数据源取行。
pub fn newMultiWayMerger<S: multiWayMergeSource>(
    source: S,
    compare: RowComparator,
) -> multiWayMerger<S> {
    multiWayMerger {
        source,
        heap: Vec::new(),
        compare,
        initialized: false,
    }
}

impl<S: multiWayMergeSource> multiWayMerger<S> {
    /// 比较堆中下标 a、b 对应行，判断 a 是否更小。
    fn less(&self, a: usize, b: usize) -> bool {
        (self.compare)(&self.heap[a].row, &self.heap[b].row).is_lt()
    }
    /// 将行上浮到堆中合适位置（sift-up）。
    fn push(&mut self, row: HeapRow) {
        self.heap.push(row);
        let mut i = self.heap.len() - 1;
        while i > 0 {
            let p = (i - 1) / 2;
            if !self.less(i, p) {
                break;
            }
            self.heap.swap(i, p);
            i = p;
        }
    }
    /// 弹出堆顶最小行，并将末元素下沉（sift-down）。
    fn pop(&mut self) -> Option<HeapRow> {
        if self.heap.is_empty() {
            return None;
        }
        let last = self.heap.pop().unwrap();
        if self.heap.is_empty() {
            return Some(last);
        }
        let first = std::mem::replace(&mut self.heap[0], last);
        let mut i = 0;
        loop {
            let l = i * 2 + 1;
            if l >= self.heap.len() {
                break;
            }
            let r = l + 1;
            // 在左右子节点中选更小者与父节点比较。
            let child = if r < self.heap.len() && self.less(r, l) {
                r
            } else {
                l
            };
            if !self.less(child, i) {
                break;
            }
            self.heap.swap(i, child);
            i = child;
        }
        Some(first)
    }
    /// 初始化：调用 source.init，并从每个非空分区取首行建堆。
    pub fn init(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        self.source.init()?;
        for id in 0..self.source.getPartitionNum() {
            if let Some(row) = self.source.next(id)? {
                self.push(HeapRow { row, partition: id });
            }
        }
        self.initialized = true;
        Ok(())
    }
    /// 取出下一行全局有序结果；同分区若还有行则补入堆。
    pub fn next(&mut self) -> Result<Option<Row>> {
        self.init()?;
        let Some(item) = self.heap.first() else {
            return Ok(None);
        };
        let partition = item.partition;
        let next_row = self.source.next(partition)?;
        let result = self
            .pop()
            .expect("heap remains non-empty after successfully fetching the successor")
            .row;
        // 从同一分区补下一行，保持堆中每分区至多一个候选。
        if let Some(row) = next_row {
            self.push(HeapRow { row, partition });
        }
        Ok(Some(result))
    }
    /// 消费全部归并结果，收集为向量。
    pub fn collect(mut self) -> Result<Vec<Row>> {
        let mut out = Vec::new();
        while let Some(row) = self.next()? {
            out.push(row);
        }
        Ok(out)
    }
}
