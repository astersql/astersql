// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// TopN 磁盘溢出（spill）辅助：内存不足时把各 worker 堆写出为 DiskRun。
//
// Spill 指算子内存超限后把中间结果落到临时存储，之后用多路归并（multi-way merge）
// 再合并出最终 TopN。状态机：`notSpilled` → `needSpill` → `inSpilling` → `notSpilled`。

use crate::sort_util::{
    DataChunk, DiskRun, MemoryTracker, Result, Row, RowComparator, SortError, inSpilling,
    needSpill, notSpilled, spillChunkSize,
};
use crate::topn_worker::topNWorker;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

/// 协调多 worker 的 spill：管理状态、磁盘 run 列表与内存/磁盘追踪。
pub struct topNSpillHelper {
    workers: Vec<Arc<Mutex<topNWorker>>>,
    /// 已写出到磁盘的有序 run（每个 run 对应一次堆导出）。
    pub sortedRowsInDisk: Vec<DiskRun>,
    compare: RowComparator,
    status: AtomicI32,
    tracker: Arc<MemoryTracker>,
    diskTracker: Arc<MemoryTracker>,
}

impl topNSpillHelper {
    /// 绑定 worker 列表、比较器与内存/磁盘追踪器。
    pub fn new(
        workers: Vec<Arc<Mutex<topNWorker>>>,
        compare: RowComparator,
        tracker: Arc<MemoryTracker>,
        disk: Arc<MemoryTracker>,
    ) -> Self {
        Self {
            workers,
            sortedRowsInDisk: Vec::new(),
            compare,
            status: AtomicI32::new(notSpilled),
            tracker,
            diskTracker: disk,
        }
    }
    /// 尝试将状态切到 `needSpill`；成功者负责真正执行 spill。
    pub fn setNeedSpill(&self) -> bool {
        self.status
            .compare_exchange(notSpilled, needSpill, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    /// 是否已有内存超限动作请求执行 spill。
    pub fn isSpillNeeded(&self) -> bool {
        self.status.load(Ordering::Acquire) == needSpill
    }
    /// 导出指定 worker 的堆为 DiskRun，并调整内存/磁盘计数。
    pub fn spillHeap(&mut self, worker_id: usize) -> Result<()> {
        let worker = self
            .workers
            .get(worker_id)
            .ok_or_else(|| SortError(format!("topn worker {worker_id} is out of range")))?;
        let mut worker = worker
            .lock()
            .map_err(|_| SortError("topn worker lock poisoned".into()))?;
        let rows = worker.heap.drainSorted();
        if rows.is_empty() {
            return Ok(());
        }
        let mut run = DiskRun::default();
        // 按 spillChunkSize 切块写入，避免单次过大的磁盘 chunk
        for part in rows.chunks(spillChunkSize.load(Ordering::Relaxed).max(1)) {
            run.add(DataChunk::new(part.to_vec()))?;
        }
        let bytes = rows.iter().map(Row::memory_usage).sum::<i64>();
        self.tracker.release(bytes);
        self.diskTracker.consume(run.memory_usage());
        self.sortedRowsInDisk.push(run);
        Ok(())
    }
    /// 对全部 worker 执行 spill，完成或失败后均回到 `notSpilled`。
    pub fn spill(&mut self) -> Result<()> {
        self.status.store(inSpilling, Ordering::Release);
        let result = (0..self.workers.len()).try_for_each(|id| self.spillHeap(id));
        // Go uses a deferred setNotSpilled, including panic/error and empty spills.
        self.status.store(notSpilled, Ordering::Release);
        result
    }
    /// 取出并清空已 spill 的 DiskRun 列表，供多路归并。
    pub fn takeRuns(&mut self) -> Vec<DiskRun> {
        std::mem::take(&mut self.sortedRowsInDisk)
    }
    /// 是否已经完成过至少一次 spill。
    pub fn isSpillTriggered(&self) -> bool {
        !self.sortedRowsInDisk.is_empty()
    }
    /// 返回 spill/归并使用的行比较器。
    pub fn compare(&self) -> RowComparator {
        self.compare.clone()
    }
}

/// 内存超限回调动作：在消耗达到限额一定比例时触发 spill。
pub struct topNSpillAction {
    helper: Arc<Mutex<topNSpillHelper>>,
    tracker: Arc<MemoryTracker>,
}
impl topNSpillAction {
    /// 绑定 spill helper 与触发用内存追踪器。
    pub fn new(helper: Arc<Mutex<topNSpillHelper>>, tracker: Arc<MemoryTracker>) -> Self {
        Self { helper, tracker }
    }
    /// 超限且 TopN 自身至少占限额 1/10 时只标记 needSpill，由执行器负责写出。
    pub fn Action(&self) -> Result<()> {
        let limit = self.tracker.bytes_limit();
        if self.tracker.exceeded() {
            let h = self
                .helper
                .lock()
                .map_err(|_| SortError("topn spill helper lock poisoned".into()))?;
            let spill_limit = limit / 10;
            if h.tracker.bytes_consumed() >= spill_limit {
                h.setNeedSpill();
            }
        }
        Ok(())
    }
}
