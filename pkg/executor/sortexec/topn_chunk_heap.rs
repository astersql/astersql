// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// TopN 有界堆：在内存中只保留排序最优的 Offset+Count 行。
//
// 使用“最差行在堆顶”的大根堆语义：堆满后，仅当新行优于堆顶（更小/更优）时才替换。
// 这是 TopN（`ORDER BY ... LIMIT`）在构建阶段的核心数据结构。

use crate::sort_util::{DataChunk, Row, RowComparator};

/// 容量为 `totalLimit` 的 TopN 有界堆，跟踪行内存占用。
pub struct topNChunkHeap {
    rows: Vec<Row>,
    totalLimit: usize,
    compare: RowComparator,
    memoryUsage: i64,
}

impl topNChunkHeap {
    /// 按总上限与行比较器构造空堆。
    pub fn new(total_limit: usize, compare: RowComparator) -> Self {
        Self {
            rows: Vec::with_capacity(total_limit),
            totalLimit: total_limit,
            compare,
            memoryUsage: 0,
        }
    }
    /// 判断下标 `a` 是否比 `b` “更差”（在比较器下更大），用于大根堆。
    fn worse(&self, a: usize, b: usize) -> bool {
        (self.compare)(&self.rows[a], &self.rows[b]).is_gt()
    }
    /// 向上调整，使更差的行上浮到堆顶方向。
    fn siftUp(&mut self, mut i: usize) {
        while i > 0 {
            let p = (i - 1) / 2;
            if !self.worse(i, p) {
                break;
            }
            self.rows.swap(i, p);
            i = p;
        }
    }
    /// 向下调整，在替换堆顶后恢复堆性质。
    fn siftDown(&mut self, mut i: usize) {
        loop {
            let l = i * 2 + 1;
            if l >= self.rows.len() {
                break;
            }
            let r = l + 1;
            let child = if r < self.rows.len() && self.worse(r, l) {
                r
            } else {
                l
            };
            if !self.worse(child, i) {
                break;
            }
            self.rows.swap(i, child);
            i = child;
        }
    }
    /// 尝试插入一行：未满则入堆；已满则仅当优于堆顶时替换。
    pub fn update(&mut self, row: Row) {
        if self.totalLimit == 0 {
            return;
        }
        let usage = row.memory_usage();
        if self.rows.len() < self.totalLimit {
            self.memoryUsage += usage;
            self.rows.push(row);
            self.siftUp(self.rows.len() - 1);
        } else if (self.compare)(&row, &self.rows[0]).is_lt() {
            // 新行优于当前最差候选：替换堆顶并下沉
            self.memoryUsage += usage - self.rows[0].memory_usage();
            self.rows[0] = row;
            self.siftDown(0);
        }
    }
    /// 将 chunk 中每一行依次 `update` 进堆。
    pub fn processChk(&mut self, chk: DataChunk) {
        for row in chk.rows {
            self.update(row);
        }
    }
    /// 堆是否已达到容量上限。
    pub fn isFull(&self) -> bool {
        self.rows.len() >= self.totalLimit
    }
    /// 当前堆内行数。
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    /// 堆内行累计估算内存（字节）。
    pub fn memoryUsage(&self) -> i64 {
        self.memoryUsage
    }
    /// 清空堆并返回此前登记的内存用量，便于上层 release。
    pub fn clear(&mut self) -> i64 {
        let old = self.memoryUsage;
        self.rows.clear();
        self.memoryUsage = 0;
        old
    }
    /// 克隆堆内容并按比较器升序排序后返回（不破坏堆）。
    pub fn sortedRows(&self) -> Vec<Row> {
        let mut rows = self.rows.clone();
        let cmp = self.compare.clone();
        rows.sort_by(|a, b| cmp(a, b));
        rows
    }
    /// 取出全部行、清空堆，并按比较器升序排序（用于 spill）。
    pub fn drainSorted(&mut self) -> Vec<Row> {
        let mut rows = std::mem::take(&mut self.rows);
        let cmp = self.compare.clone();
        rows.sort_by(|a, b| cmp(a, b));
        self.memoryUsage = 0;
        rows
    }
    /// 收缩容量，避免长期超过 `totalLimit` 的多余预留。
    pub fn compact(&mut self) {
        self.rows.shrink_to(self.totalLimit.max(self.rows.len()));
    }
}
