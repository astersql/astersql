// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 排序分区（SortPartition）：串行排序路径中的一块可 spill 的有序数据单元。
//
// 分区先在内存中累积行；内存压力下将已排序行写入 [`DiskRun`]（落盘），
// 再通过游标按序吐出行。状态机：`notSpilled` → `inSpilling` → `spillTriggered`。

use crate::sort_util::{
    DataChunk, DiskRun, MemoryTracker, Result, Row, RowComparator, SortError, errSpillEmptyChunk,
    inSpilling, notSpilled, signalCheckpointForSort, spillChunkSize, spillTriggered,
};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

struct SortCancelled;

/// 单个排序分区：内存行缓冲 + 可选磁盘 run + spill 状态。
pub struct SortPartition {
    compare: RowComparator,
    savedRows: Vec<Row>,
    cursor: usize,
    inDisk: Option<DiskRun>,
    diskRows: Vec<Row>,
    memoryBytes: i64,
    diskBytes: i64,
    sorted: bool,
    closed: bool,
    spillStatus: AtomicI32,
    spillError: Option<SortError>,
    killed: Arc<AtomicBool>,
    memTracker: Arc<MemoryTracker>,
    diskTracker: Arc<MemoryTracker>,
}

impl SortPartition {
    /// 构造空分区，初始 spill 状态为 notSpilled。
    pub fn new(compare: RowComparator, mem: Arc<MemoryTracker>, disk: Arc<MemoryTracker>) -> Self {
        Self::newWithKiller(compare, Arc::new(AtomicBool::new(false)), mem, disk)
    }

    /// 构造带查询取消信号的分区；排序与 spill 均会传播中断错误。
    pub fn newWithKiller(
        compare: RowComparator,
        killed: Arc<AtomicBool>,
        mem: Arc<MemoryTracker>,
        disk: Arc<MemoryTracker>,
    ) -> Self {
        Self {
            compare,
            savedRows: Vec::new(),
            cursor: 0,
            inDisk: None,
            diskRows: Vec::new(),
            memoryBytes: 0,
            diskBytes: 0,
            sorted: false,
            closed: false,
            spillStatus: AtomicI32::new(notSpilled),
            spillError: None,
            killed,
            memTracker: mem,
            diskTracker: disk,
        }
    }
    /// 向分区追加 chunk；已关闭或正在/已完成 spill 时返回 false。
    pub fn add(&mut self, chk: DataChunk) -> bool {
        if self.closed || self.spillStatus.load(Ordering::Acquire) >= inSpilling {
            return false;
        }
        let usage = chk.memory_usage();
        self.memTracker.consume(usage);
        self.memoryBytes += usage;
        self.savedRows.extend(chk.rows);
        self.sorted = false;
        true
    }
    /// 剩余可读行数（内存未读 + 磁盘未读）。
    pub fn numRows(&self) -> usize {
        self.savedRows.len() + self.diskRows.len().saturating_sub(self.cursor)
    }
    /// 当前内存中保存行的估算占用字节数。
    pub fn memoryUsage(&self) -> i64 {
        self.memoryBytes
    }
    /// 若尚未排序则对 `savedRows` 原地排序；若先前 spill 出错则返回该错误。
    pub fn sort(&mut self) -> Result<()> {
        if let Some(err) = self.spillError.clone() {
            return Err(err);
        }
        if !self.sorted {
            let cmp = self.compare.clone();
            let killed = self.killed.clone();
            let mut comparisons = 0;
            let sort_result = catch_unwind(AssertUnwindSafe(|| {
                self.savedRows.sort_by(|a, b| {
                    if comparisons >= signalCheckpointForSort {
                        if killed.load(Ordering::Acquire) {
                            std::panic::panic_any(SortCancelled);
                        }
                        comparisons = 0;
                    }
                    comparisons += 1;
                    cmp(a, b)
                });
            }));
            if let Err(payload) = sort_result {
                if payload.is::<SortCancelled>() {
                    return Err(SortError("query interrupted".into()));
                }
                resume_unwind(payload);
            }
            self.sorted = true;
        }
        Ok(())
    }
    /// 将内存有序行按 spillChunkSize 切块写入 DiskRun，并切换内存/磁盘追踪用量。
    pub fn spillToDisk(&mut self) -> Result<()> {
        if self.spillStatus.load(Ordering::Acquire) == spillTriggered {
            return Ok(());
        }
        self.sort()?;
        self.spillStatus.store(inSpilling, Ordering::Release);

        let result = if self.closed {
            Ok(())
        } else if self.savedRows.is_empty() {
            Err(errSpillEmptyChunk())
        } else {
            (|| {
                let chunk_size = spillChunkSize.load(Ordering::Relaxed).max(1);
                let mut run = DiskRun::default();
                // 按配置的 spill chunk 大小切分写入磁盘 run
                for rows in self.savedRows.chunks(chunk_size) {
                    if self.killed.load(Ordering::Acquire) {
                        return Err(SortError("query interrupted".into()));
                    }
                    run.add(DataChunk::new(rows.to_vec()))?;
                }
                let memory = self.memoryBytes;
                let disk_bytes = run.memory_usage();
                self.diskTracker.consume(disk_bytes);
                self.memTracker.release(memory);
                self.memoryBytes = 0;
                self.diskBytes = disk_bytes;
                self.diskRows = run.clone().into_rows();
                self.inDisk = Some(run);
                self.savedRows.clear();
                self.cursor = 0;
                Ok(())
            })()
        };
        self.spillStatus.store(spillTriggered, Ordering::Release);
        result
    }
    /// 取出下一行有序行（优先磁盘 run，否则内存）；耗尽返回 None。
    pub fn getNextSortedRow(&mut self) -> Result<Option<Row>> {
        self.sort()?;
        let rows = if self.inDisk.is_some() {
            &self.diskRows
        } else {
            &self.savedRows
        };
        let result = rows.get(self.cursor).cloned();
        if result.is_some() {
            self.cursor += 1;
        }
        Ok(result)
    }
    /// 记录 spill 过程中的错误，供后续 sort 调用返回。
    pub fn setSpillError(&mut self, err: SortError) {
        self.spillError = Some(err);
    }
    /// 当前 spill 状态码（notSpilled / needSpill / inSpilling / spillTriggered）。
    pub fn spillStatus(&self) -> i32 {
        self.spillStatus.load(Ordering::Acquire)
    }
    /// 释放内存与磁盘追踪用量，关闭 DiskRun 并标记 closed。
    pub fn close(&mut self) {
        self.memTracker.release(self.memoryBytes);
        if let Some(run) = &mut self.inDisk {
            self.diskTracker.release(self.diskBytes);
            run.close();
        }
        self.memoryBytes = 0;
        self.diskBytes = 0;
        self.savedRows.clear();
        self.diskRows.clear();
        self.closed = true;
    }
}
