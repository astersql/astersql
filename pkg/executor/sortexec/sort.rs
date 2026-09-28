// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 排序执行器（SortExec）：对应 SQL `ORDER BY` 的物化排序算子。
//
// 从子算子拉取 chunk，按 `ByItems`（排序键）比较后产出有序结果。
// `concurrency == 1` 走串行分区路径（可按内存限额 spill 到磁盘）；
// `concurrency > 1` 走并行 worker 路径，必要时由 [`parallelSortSpillHelper`] 落盘。

use crate::multi_way_merge::{newMultiWayMerger, sortPartitionSource};
use crate::parallel_sort_spill_helper::parallelSortSpillHelper;
use crate::parallel_sort_worker::parallelSortWorker;
use crate::sort_partition::SortPartition;
use crate::sort_util::{
    DataChunk, MemoryTracker, Result, Row, RowComparator, SortError, SortKey, comparator,
};
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// 行数据源：Open/Next/Close 生命周期，供 SortExec 拉取输入 chunk。
pub trait RowSource: Send {
    /// 打开数据源；默认无操作。
    fn open(&mut self) -> Result<()> {
        Ok(())
    }
    /// 拉取下一个 chunk；耗尽时返回 None。
    fn next(&mut self) -> Result<Option<DataChunk>>;
    /// 关闭数据源；默认无操作。
    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

/// 基于内存队列的测试/简易数据源。
pub struct VecRowSource {
    chunks: VecDeque<DataChunk>,
}
impl VecRowSource {
    /// 由 chunk 列表构造数据源，按 FIFO 顺序产出。
    pub fn new(chunks: Vec<DataChunk>) -> Self {
        Self {
            chunks: VecDeque::from(chunks),
        }
    }
}
impl RowSource for VecRowSource {
    fn next(&mut self) -> Result<Option<DataChunk>> {
        Ok(self.chunks.pop_front())
    }
}

/// 排序执行器：串行分区或多 worker 并行排序，支持内存追踪与 spill。
pub struct SortExec {
    child: Box<dyn RowSource>,
    byItems: Vec<SortKey>,
    compare: RowComparator,
    concurrency: usize,
    maxChunkSize: usize,
    memLimit: i64,
    memTracker: Arc<MemoryTracker>,
    diskTracker: Arc<MemoryTracker>,
    partitions: Vec<Arc<Mutex<SortPartition>>>,
    spillHelper: Option<parallelSortSpillHelper>,
    result: VecDeque<Row>,
    fetched: bool,
    opened: bool,
    killed: Arc<AtomicBool>,
}

impl SortExec {
    /// 构造排序执行器；比较器由 `by_items` 生成，concurrency/chunk 至少为 1。
    pub fn new(
        child: Box<dyn RowSource>,
        by_items: Vec<SortKey>,
        concurrency: usize,
        max_chunk_size: usize,
        mem_limit: i64,
    ) -> Self {
        let compare = comparator(by_items.clone());
        Self {
            child,
            byItems: by_items,
            compare,
            concurrency: concurrency.max(1),
            maxChunkSize: max_chunk_size.max(1),
            memLimit: mem_limit,
            memTracker: Arc::new(MemoryTracker::new(mem_limit)),
            diskTracker: Arc::new(MemoryTracker::new(-1)),
            partitions: Vec::new(),
            spillHelper: None,
            result: VecDeque::new(),
            fetched: false,
            opened: false,
            killed: Arc::new(AtomicBool::new(false)),
        }
    }
    /// 打开子算子；要求至少有一个排序键。
    pub fn Open(&mut self) -> Result<()> {
        if self.opened {
            return Ok(());
        }
        if self.byItems.is_empty() {
            return Err(SortError("sort requires at least one ordering item".into()));
        }
        self.child.open()?;
        self.opened = true;
        Ok(())
    }
    /// 串行路径：按内存限额切分 SortPartition，必要时 spill，再多路归并。
    fn fetchUnparallel(&mut self) -> Result<()> {
        let mut current = Arc::new(Mutex::new(SortPartition::newWithKiller(
            self.compare.clone(),
            self.killed.clone(),
            self.memTracker.clone(),
            self.diskTracker.clone(),
        )));
        while let Some(chunk) = self.child.next()? {
            if self.killed.load(std::sync::atomic::Ordering::Acquire) {
                return Err(SortError("query interrupted".into()));
            }
            let usage = chunk.memory_usage();
            // 当前分区已有数据且加上新 chunk 将超限时，先 spill 并开启新分区
            if self.memLimit >= 0
                && self.memTracker.bytes_consumed() + usage >= self.memLimit
                && current
                    .lock()
                    .map_err(|_| SortError("sort partition lock poisoned".into()))?
                    .numRows()
                    > 0
            {
                current
                    .lock()
                    .map_err(|_| SortError("sort partition lock poisoned".into()))?
                    .spillToDisk()?;
                self.partitions.push(current);
                current = Arc::new(Mutex::new(SortPartition::newWithKiller(
                    self.compare.clone(),
                    self.killed.clone(),
                    self.memTracker.clone(),
                    self.diskTracker.clone(),
                )));
            }
            if !current
                .lock()
                .map_err(|_| SortError("sort partition lock poisoned".into()))?
                .add(chunk)
            {
                return Err(crate::sort_util::errFailToAddChunk());
            }
        }
        if current
            .lock()
            .map_err(|_| SortError("sort partition lock poisoned".into()))?
            .numRows()
            > 0
        {
            self.partitions.push(current);
        }
        if !self.partitions.is_empty() {
            self.result = VecDeque::from(
                newMultiWayMerger(
                    sortPartitionSource::new(self.partitions.clone()),
                    self.compare.clone(),
                )
                .collect()?,
            );
        }
        Ok(())
    }
    /// 并行路径：轮询分发 chunk 给各 worker，超限时触发 spill，最后 mergeAll。
    fn fetchParallel(&mut self) -> Result<()> {
        let workers: Vec<_> = (0..self.concurrency)
            .map(|_| {
                Arc::new(Mutex::new(parallelSortWorker::new(
                    self.compare.clone(),
                    self.maxChunkSize * 8,
                    self.killed.clone(),
                    self.memTracker.clone(),
                )))
            })
            .collect();
        let mut index = 0;
        while let Some(chunk) = self.child.next()? {
            if self.killed.load(std::sync::atomic::Ordering::Acquire) {
                return Err(SortError("query interrupted".into()));
            }
            workers[index % workers.len()]
                .lock()
                .map_err(|_| SortError("parallel sort worker lock poisoned".into()))?
                .saveChunk(chunk)?;
            index += 1;
            // 内存超限时惰性创建 spill helper 并尝试落盘
            if self.memTracker.exceeded() {
                if self.spillHelper.is_none() {
                    self.spillHelper = Some(parallelSortSpillHelper::new(
                        workers.clone(),
                        self.compare.clone(),
                        self.memTracker.clone(),
                        self.diskTracker.clone(),
                    ));
                }
                let helper = self.spillHelper.as_mut().unwrap();
                if helper.setNeedSpill() {
                    helper.spill()?;
                }
            }
        }
        let mut helper = self.spillHelper.take().unwrap_or_else(|| {
            parallelSortSpillHelper::new(
                workers,
                self.compare.clone(),
                self.memTracker.clone(),
                self.diskTracker.clone(),
            )
        });
        self.result = VecDeque::from(helper.mergeAll()?);
        self.spillHelper = Some(helper);
        Ok(())
    }
    /// 一次性拉取并完成排序；按 concurrency 选择串行或并行路径。
    fn fetch(&mut self) -> Result<()> {
        if self.fetched {
            return Ok(());
        }
        if self.concurrency > 1 {
            self.fetchParallel()?;
        } else {
            self.fetchUnparallel()?;
        }
        self.fetched = true;
        Ok(())
    }
    /// 返回至多 `max_rows`（且不超过 maxChunkSize）的有序行 chunk。
    pub fn Next(&mut self, max_rows: usize) -> Result<DataChunk> {
        self.Open()?;
        self.fetch()?;
        let n = max_rows.max(1).min(self.maxChunkSize);
        let mut rows = Vec::with_capacity(n);
        while rows.len() < n {
            let Some(row) = self.result.pop_front() else {
                break;
            };
            rows.push(row);
        }
        Ok(DataChunk::new(rows))
    }
    /// 关闭分区、清空结果与 spill 状态，并关闭子算子。
    pub fn Close(&mut self) -> Result<()> {
        for p in &self.partitions {
            p.lock()
                .map_err(|_| SortError("sort partition lock poisoned".into()))?
                .close();
        }
        self.partitions.clear();
        self.result.clear();
        self.spillHelper = None;
        self.diskTracker.release(self.diskTracker.bytes_consumed());
        self.fetched = false;
        self.opened = false;
        self.child.close()
    }
    /// 中断当前排序；下一次拉取或 worker 检查点会返回中断错误。
    pub fn Kill(&self) {
        self.killed
            .store(true, std::sync::atomic::Ordering::Release);
    }
    /// 是否已触发磁盘 spill（磁盘追踪器有用量或 helper 标记已触发）。
    pub fn IsSpillTriggered(&self) -> bool {
        self.diskTracker.bytes_consumed() > 0
            || self
                .spillHelper
                .as_ref()
                .is_some_and(parallelSortSpillHelper::isSpillTriggered)
    }
    /// 串行路径下当前分区列表长度。
    pub fn GetPartitionListLen(&self) -> usize {
        self.partitions.len()
    }
    /// 返回内存用量追踪器。
    pub fn GetMemTracker(&self) -> &Arc<MemoryTracker> {
        &self.memTracker
    }
    /// 返回磁盘用量追踪器。
    pub fn GetDiskTracker(&self) -> &Arc<MemoryTracker> {
        &self.diskTracker
    }
}
