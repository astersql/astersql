// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 并行排序（parallel sort）的 spill（落盘）助手。
//
// 在内存压力下将各 worker 已多路归并的有序行写入 DiskRun，再与残留内存数据做最终归并。
// 状态机：`notSpilled` → `needSpill` → `inSpilling` → `spillTriggered`。

use crate::multi_way_merge::{diskSource, newMultiWayMerger};
use crate::parallel_sort_worker::parallelSortWorker;
use crate::sort_util::{
    DataChunk, DiskRun, MemoryTracker, Result, Row, RowComparator, SortError, inSpilling,
    needSpill, notSpilled, spillChunkSize,
};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

/// 协调并行排序 worker 的落盘与最终多路归并。
pub struct parallelSortSpillHelper {
    /// 参与并行排序的 worker 列表。
    pub workers: Vec<Arc<Mutex<parallelSortWorker>>>,
    /// 已落盘的有序 DiskRun 列表。
    pub sortedRowsInDisk: Vec<DiskRun>,
    /// 行比较器，用于归并磁盘 run。
    compare: RowComparator,
    /// spill 状态（原子）：未落盘 / 需落盘 / 落盘中 / 已触发。
    status: AtomicI32,
    /// 内存用量跟踪器。
    memTracker: Arc<MemoryTracker>,
    /// 磁盘用量跟踪器。
    diskTracker: Arc<MemoryTracker>,
    /// 若先前 spill 失败，缓存错误以便后续调用返回。
    spillError: Option<SortError>,
}

impl parallelSortSpillHelper {
    /// 构造 spill 助手，初始状态为 notSpilled。
    pub fn new(
        workers: Vec<Arc<Mutex<parallelSortWorker>>>,
        compare: RowComparator,
        mem: Arc<MemoryTracker>,
        disk: Arc<MemoryTracker>,
    ) -> Self {
        Self {
            workers,
            sortedRowsInDisk: Vec::new(),
            compare,
            status: AtomicI32::new(notSpilled),
            memTracker: mem,
            diskTracker: disk,
            spillError: None,
        }
    }
    /// 尝试将状态从 notSpilled 置为 needSpill。
    pub fn setNeedSpill(&self) -> bool {
        self.status
            .compare_exchange(notSpilled, needSpill, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    /// 是否已经产生至少一个磁盘 run。
    pub fn isSpillTriggered(&self) -> bool {
        !self.sortedRowsInDisk.is_empty()
    }
    /// 返回当前 spill 状态码。
    pub fn spillStatus(&self) -> i32 {
        self.status.load(Ordering::Acquire)
    }
    /// 执行落盘：对各 worker 做多路归并，按 spillChunkSize 切块写入 DiskRun，再合并为一条 run。
    pub fn spill(&mut self) -> Result<()> {
        if let Some(err) = self.spillError.clone() {
            return Err(err);
        }
        self.status.store(inSpilling, Ordering::Release);
        let result = (|| {
            let mut runs = Vec::new();
            for worker in &self.workers {
                let mut worker = worker
                    .lock()
                    .map_err(|_| SortError("parallel sort worker lock poisoned".into()))?;
                // 先在 worker 内多路归并得到有序行，再写入磁盘。
                let rows = worker.multiWayMerge()?;
                if rows.is_empty() {
                    continue;
                }
                let mut run = DiskRun::default();
                // 按 spillChunkSize 切分为 DataChunk，避免单次 IO 过大。
                for part in rows.chunks(spillChunkSize.load(Ordering::Relaxed).max(1)) {
                    run.add(DataChunk::new(part.to_vec()))?;
                }
                self.diskTracker.consume(run.memory_usage());
                runs.push(run);
            }
            if !runs.is_empty() {
                // 多个 worker 的 run 再归并为一条有序磁盘 run。
                self.sortedRowsInDisk.push(self.mergeRuns(runs)?);
            }
            Ok(())
        })();
        // 对齐 Go defer：正常返回和可传播错误均恢复为 notSpilled。
        self.status.store(notSpilled, Ordering::Release);
        result
    }
    /// 将多个 DiskRun 经多路归并合并为一条 DiskRun。
    fn mergeRuns(&self, runs: Vec<DiskRun>) -> Result<DiskRun> {
        let rows = newMultiWayMerger(diskSource::new(runs), self.compare.clone()).collect()?;
        let mut merged = DiskRun::default();
        for part in rows.chunks(spillChunkSize.load(Ordering::Relaxed).max(1)) {
            merged.add(DataChunk::new(part.to_vec()))?;
        }
        Ok(merged)
    }
    /// 最终合并：若仍 needSpill 则先落盘，再将磁盘 run 与 worker 残留内存一并多路归并。
    pub fn mergeAll(&mut self) -> Result<Vec<Row>> {
        if self.status.load(Ordering::Acquire) == needSpill {
            self.spill()?;
        }
        let mut runs = std::mem::take(&mut self.sortedRowsInDisk);
        for worker in &self.workers {
            let rows = worker
                .lock()
                .map_err(|_| SortError("parallel sort worker lock poisoned".into()))?
                .multiWayMerge()?;
            if !rows.is_empty() {
                let mut run = DiskRun::default();
                run.add(DataChunk::new(rows))?;
                runs.push(run);
            }
        }
        if runs.is_empty() {
            return Ok(Vec::new());
        }
        newMultiWayMerger(diskSource::new(runs), self.compare.clone()).collect()
    }
    /// 返回内存跟踪器引用。
    pub fn memoryTracker(&self) -> &Arc<MemoryTracker> {
        &self.memTracker
    }
}
