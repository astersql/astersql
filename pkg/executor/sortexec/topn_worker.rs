// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// TopN 并行 worker：把输入 chunk 喂给有界堆，并同步内存用量。
//
// TopN（取排序后的前 N 行，常对应 SQL `ORDER BY ... LIMIT`）在并行执行时，
// 每个 worker 维护一份独立的 [`topNChunkHeap`]，由上层 [`TopNExec`] 轮询分发 chunk。

use crate::sort_util::{DataChunk, MemoryTracker, Result, SortError};
use crate::topn_chunk_heap::topNChunkHeap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 单个 TopN worker：持有有界堆、取消标志与内存追踪。
pub struct topNWorker {
    /// 当前 worker 的 TopN 有界堆（大根堆，堆顶为最差候选）。
    pub heap: topNChunkHeap,
    killed: Arc<AtomicBool>,
    tracker: Arc<MemoryTracker>,
    processedRows: usize,
}
impl topNWorker {
    /// 构造 worker，绑定共享的取消标志与内存追踪器。
    pub fn new(heap: topNChunkHeap, killed: Arc<AtomicBool>, tracker: Arc<MemoryTracker>) -> Self {
        Self {
            heap,
            killed,
            tracker,
            processedRows: 0,
        }
    }
    /// 处理一个输入 chunk：检查中断后更新堆，并按增量消费内存配额。
    pub fn run(&mut self, chk: DataChunk) -> Result<()> {
        // 查询被 kill 时立即返回，避免继续占用 CPU/内存
        if self.killed.load(Ordering::Acquire) {
            return Err(SortError("query interrupted".into()));
        }
        let old = self.heap.memoryUsage();
        self.processedRows += chk.num_rows();
        self.heap.processChk(chk);
        self.tracker.consume(self.heap.memoryUsage() - old);
        Ok(())
    }
    /// 清空堆并归还已登记的内存用量，供 Close/复用。
    pub fn reset(&mut self) {
        let usage = self.heap.clear();
        self.tracker.release(usage);
        self.processedRows = 0;
    }
    /// 返回本 worker 已处理的行数。
    pub fn processedRows(&self) -> usize {
        self.processedRows
    }
}
