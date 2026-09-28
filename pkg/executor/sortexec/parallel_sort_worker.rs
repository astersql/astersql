// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 并行排序（parallel sort）的 worker：接收 chunk、本地排序并多路归并。
//
// 每个 worker 独立缓存输入 chunk，在批次满或需要归并时排序，
// 再通过 [`memorySource`] + 多路归并器产出本 worker 的有序行序列。
// 排序途中按检查点轮询 `killed`，以支持查询中断（kill）。

use crate::multi_way_merge::{memorySource, newMultiWayMerger};
use crate::sort_util::{DataChunk, MemoryTracker, Result, Row, RowComparator, SortError};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct SortCancelled;

/// 本地排序时每隔多少次行比较检查一次 kill 信号，避免长时间阻塞不可取消。
pub const parallelSortCheckSignalCheckpoint: usize = 20_000;

/// 单个并行排序 worker：持有未排序 chunk、已排序批次、比较器与内存追踪。
pub struct parallelSortWorker {
    /// 尚未排序的输入 chunk 缓冲。
    pub chunks: Vec<DataChunk>,
    /// 已完成本地排序的行批次列表，供后续多路归并。
    pub localSortedRows: Vec<Vec<Row>>,
    compare: RowComparator,
    maxRowsInBatch: usize,
    killed: Arc<AtomicBool>,
    memTracker: Arc<MemoryTracker>,
    memoryBytes: i64,
    timesOfRowCompare: usize,
}

impl parallelSortWorker {
    /// 构造 worker；`max_rows` 至少为 1，作为触发 `sortBatch` 的行数阈值。
    pub fn new(
        compare: RowComparator,
        max_rows: usize,
        killed: Arc<AtomicBool>,
        mem: Arc<MemoryTracker>,
    ) -> Self {
        Self {
            chunks: Vec::new(),
            localSortedRows: Vec::new(),
            compare,
            maxRowsInBatch: max_rows.max(1),
            killed,
            memTracker: mem,
            memoryBytes: 0,
            timesOfRowCompare: 0,
        }
    }
    /// 缓存一个输入 chunk；若查询已被 kill 则立即返回错误。
    pub fn saveChunk(&mut self, chunk: DataChunk) -> Result<()> {
        // 查询中断时拒绝继续囤积数据
        if self.killed.load(Ordering::Acquire) {
            return Err(SortError("query interrupted".into()));
        }
        let usage = chunk.memory_usage();
        self.memTracker.consume(usage);
        self.memoryBytes += usage;
        self.chunks.push(chunk);
        Ok(())
    }
    /// 将缓冲 chunk 展平为行并原地排序，结果追加到 `localSortedRows`。
    pub fn sortBatch(&mut self) -> Result<()> {
        if self.chunks.is_empty() {
            return Ok(());
        }
        let mut rows: Vec<Row> = self.chunks.drain(..).flat_map(|c| c.rows).collect();
        let cmp = self.compare.clone();
        let killed = self.killed.clone();
        let comparisons = &mut self.timesOfRowCompare;
        // 与 Go 的 keyColumnsLess 一致，在真实比较次数达到检查点时轮询 kill。
        // sort_by 不支持可失败比较器，因此仅用私有哨兵穿过排序 API，并在边界恢复为 Result。
        let sort_result = catch_unwind(AssertUnwindSafe(|| {
            rows.sort_by(|a, b| {
                if *comparisons >= parallelSortCheckSignalCheckpoint {
                    if killed.load(Ordering::Acquire) {
                        std::panic::panic_any(SortCancelled);
                    }
                    *comparisons = 0;
                }
                *comparisons += 1;
                cmp(a, b)
            });
        }));
        if let Err(payload) = sort_result {
            if payload.is::<SortCancelled>() {
                return Err(SortError("query interrupted".into()));
            }
            resume_unwind(payload);
        }
        self.localSortedRows.push(rows);
        Ok(())
    }
    /// 当缓冲行数达到批次上限或仍有未排序 chunk 时触发 `sortBatch`。
    pub fn sortLocal(&mut self) -> Result<()> {
        let rows = self.chunks.iter().map(DataChunk::num_rows).sum::<usize>();
        if rows >= self.maxRowsInBatch || !self.chunks.is_empty() {
            self.sortBatch()?;
        }
        Ok(())
    }
    /// 先完成本地排序，再对所有有序批次做内存多路归并，得到全局有序行。
    pub fn multiWayMerge(&mut self) -> Result<Vec<Row>> {
        self.sortLocal()?;
        let parts = std::mem::take(&mut self.localSortedRows);
        let rows = newMultiWayMerger(memorySource::new(parts), self.compare.clone()).collect()?;
        self.memTracker.release(self.memoryBytes);
        self.memoryBytes = 0;
        Ok(rows)
    }
    /// 清空 chunk 与已排序批次，供复用或 Close。
    pub fn reset(&mut self) {
        self.chunks.clear();
        self.localSortedRows.clear();
        self.memTracker.release(self.memoryBytes);
        self.memoryBytes = 0;
    }
    /// 向内存追踪器归还指定字节数。
    pub fn releaseMemory(&self, bytes: i64) {
        self.memTracker.release(bytes);
    }
    /// 统计未排序 chunk 与已排序批次中的总行数。
    pub fn rowCount(&self) -> usize {
        self.chunks.iter().map(DataChunk::num_rows).sum::<usize>()
            + self.localSortedRows.iter().map(Vec::len).sum::<usize>()
    }
}
