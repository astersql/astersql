// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// TopN 执行器：对子节点输出做排序并截取 Offset/Count 窗口。
//
// TopN 对应 SQL `ORDER BY ... LIMIT offset, count`。实现上用多 worker 并行维护有界堆；
// 内存超限时经 spill 落到磁盘，最后多路归并得到有序结果，再应用 Limit。
// Rank TopN 用截断前缀键扩大内部候选集，但最终输出仍严格遵循 Offset/Count。

use crate::multi_way_merge::{diskSource, newMultiWayMerger};
use crate::sort::RowSource;
use crate::sort_util::{
    DataChunk, DiskRun, MemoryTracker, Result, Row, RowComparator, SortError, SortKey, comparator,
};
use crate::topn_chunk_heap::topNChunkHeap;
use crate::topn_spill::topNSpillHelper;
use crate::topn_worker::topNWorker;
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// LIMIT 的偏移与条数（对应 SQL `LIMIT offset, count`）。
#[derive(Clone, Copy, Debug, Default)]
pub struct Limit {
    /// 跳过的前缀行数。
    pub Offset: usize,
    /// 最多返回的行数。
    pub Count: usize,
}

/// RankTopN 候选扩展所需的前缀排序键与期望窗口大小。
#[derive(Clone, Debug)]
pub struct RankInfo {
    /// 判定“并列”时使用的前缀排序键。
    pub prefixKeys: Vec<SortKey>,
    /// 期望纳入的名次规模（可大于 Limit.Count 以容纳并列）。
    pub expectedCount: usize,
}

/// TopN 物理执行器：并行堆构建、可选 spill、Limit/Rank 截取与输出投影。
pub struct TopNExec {
    child: Box<dyn RowSource>,
    byItems: Vec<SortKey>,
    compare: RowComparator,
    pub limit: Limit,
    rankInfo: Option<RankInfo>,
    columnIdxsUsedByChild: Option<Vec<usize>>,
    concurrency: usize,
    maxChunkSize: usize,
    memTracker: Arc<MemoryTracker>,
    diskTracker: Arc<MemoryTracker>,
    workers: Vec<Arc<Mutex<topNWorker>>>,
    spillHelper: Option<topNSpillHelper>,
    result: VecDeque<Row>,
    fetched: bool,
    opened: bool,
    killed: Arc<AtomicBool>,
}

impl TopNExec {
    /// 构造 TopN：创建 concurrency 个 worker，各自带容量为 Offset+Count 的有界堆。
    pub fn new(
        child: Box<dyn RowSource>,
        by_items: Vec<SortKey>,
        limit: Limit,
        rank_info: Option<RankInfo>,
        concurrency: usize,
        max_chunk_size: usize,
        mem_limit: i64,
    ) -> Self {
        let compare = comparator(by_items.clone());
        let mem = Arc::new(MemoryTracker::new(mem_limit));
        let killed = Arc::new(AtomicBool::new(false));
        // 每个 worker 独立堆，容量覆盖 offset+count 候选
        let workers = (0..concurrency.max(1))
            .map(|_| {
                Arc::new(Mutex::new(topNWorker::new(
                    topNChunkHeap::new(limit.Offset.saturating_add(limit.Count), compare.clone()),
                    killed.clone(),
                    mem.clone(),
                )))
            })
            .collect();
        Self {
            child,
            byItems: by_items,
            compare,
            limit,
            rankInfo: rank_info,
            columnIdxsUsedByChild: None,
            concurrency: concurrency.max(1),
            maxChunkSize: max_chunk_size.max(1),
            memTracker: mem,
            diskTracker: Arc::new(MemoryTracker::new(-1)),
            workers,
            spillHelper: None,
            result: VecDeque::new(),
            fetched: false,
            opened: false,
            killed,
        }
    }
    /// Configures Go's inline projection path. Ordering expressions continue
    /// to see the full child row; projection is applied only to TopN output.
    /// 配置内联投影列：排序仍看完整子行，仅对 TopN 输出做列裁剪。
    pub fn SetColumnIdxsUsedByChild(&mut self, columns: Vec<usize>) {
        self.columnIdxsUsedByChild = Some(columns);
    }
    /// 打开子节点；要求至少有一个排序项。
    pub fn Open(&mut self) -> Result<()> {
        if !self.opened {
            if self.byItems.is_empty() {
                return Err(SortError("topn requires at least one ordering item".into()));
            }
            self.child.open()?;
            self.opened = true;
        }
        Ok(())
    }
    /// 标准 TopN：轮询分发 chunk 给 worker，必要时 spill，再多路归并截断。
    fn fetchTopN(&mut self) -> Result<Vec<Row>> {
        if self.limit.Count == 0 {
            return Ok(Vec::new());
        }
        let mut idx = 0;
        while let Some(chunk) = self.child.next()? {
            self.workers[idx % self.workers.len()]
                .lock()
                .map_err(|_| SortError("topn worker lock poisoned".into()))?
                .run(chunk)?;
            idx += 1;
            // 内存超限：懒创建 spill helper，并由成功抢到 needSpill 的路径执行写出
            if self.memTracker.exceeded() {
                if self.spillHelper.is_none() {
                    self.spillHelper = Some(topNSpillHelper::new(
                        self.workers.clone(),
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
        // 汇总磁盘 run，并附上各 worker 内存堆中尚未 spill 的有序行
        let mut runs = self
            .spillHelper
            .as_mut()
            .map(topNSpillHelper::takeRuns)
            .unwrap_or_default();
        for worker in &self.workers {
            let rows = worker
                .lock()
                .map_err(|_| SortError("topn worker lock poisoned".into()))?
                .heap
                .sortedRows();
            if !rows.is_empty() {
                let mut run = DiskRun::default();
                run.add(DataChunk::new(rows))?;
                runs.push(run);
            }
        }
        // 多路归并各 run，得到全局有序候选
        let mut rows = if runs.is_empty() {
            Vec::new()
        } else {
            newMultiWayMerger(diskSource::new(runs), self.compare.clone()).collect()?
        };
        let total = self.limit.Offset.saturating_add(self.limit.Count);
        if rows.len() > total {
            rows.truncate(total);
        }
        Ok(rows)
    }
    /// 严格应用 Offset/Count。RankInfo 只用于扩大内部候选集，不能改变最终 LIMIT。
    fn applyLimitAndRank(&self, rows: Vec<Row>) -> Vec<Row> {
        if self.limit.Offset >= rows.len() {
            return Vec::new();
        }
        let start = self.limit.Offset;
        let end = start.saturating_add(self.limit.Count).min(rows.len());
        rows[start..end].to_vec()
    }
    /// Rank 路径：全量拉取后整体排序，再由 `applyLimitAndRank` 截取。
    fn fetchRankTopN(&mut self) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        while let Some(chunk) = self.child.next()? {
            rows.extend(chunk.rows);
        }
        let cmp = self.compare.clone();
        rows.sort_by(|a, b| cmp(a, b));
        Ok(rows)
    }
    /// 惰性物化：按是否 Rank 选择路径，结果写入 `result` 队列。
    fn fetch(&mut self) -> Result<()> {
        if self.fetched {
            return Ok(());
        }
        let rows = if self.rankInfo.is_some() {
            self.fetchRankTopN()?
        } else {
            self.fetchTopN()?
        };
        self.result = VecDeque::from(self.applyLimitAndRank(rows));
        self.fetched = true;
        Ok(())
    }
    /// 打开并物化后，弹出至多 `max_rows`/`maxChunkSize` 行；可做内联投影。
    pub fn Next(&mut self, max_rows: usize) -> Result<DataChunk> {
        self.Open()?;
        self.fetch()?;
        let mut rows = Vec::new();
        let cap = max_rows.max(1).min(self.maxChunkSize);
        while rows.len() < cap {
            let Some(row) = self.result.pop_front() else {
                break;
            };
            // 仅输出需要的列，排序阶段仍使用完整行
            if let Some(columns) = &self.columnIdxsUsedByChild {
                let projected = columns
                    .iter()
                    .map(|index| {
                        row.0.get(*index).cloned().ok_or_else(|| {
                            SortError(format!(
                                "topn inline projection column index {index} exceeds row width {}",
                                row.0.len()
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                rows.push(Row(projected));
            } else {
                rows.push(row);
            }
        }
        Ok(DataChunk::new(rows))
    }
    /// 重置 worker/结果/spill 状态并关闭子节点。
    pub fn Close(&mut self) -> Result<()> {
        for worker in &self.workers {
            worker
                .lock()
                .map_err(|_| SortError("topn worker lock poisoned".into()))?
                .reset();
        }
        self.result.clear();
        self.spillHelper = None;
        self.diskTracker.release(self.diskTracker.bytes_consumed());
        self.fetched = false;
        self.opened = false;
        self.child.close()
    }
    /// 中断当前 TopN；下一次 worker 检查会返回中断错误。
    pub fn Kill(&self) {
        self.killed
            .store(true, std::sync::atomic::Ordering::Release);
    }
    /// 是否已触发磁盘 spill（磁盘用量或 helper 状态）。
    pub fn IsSpillTriggered(&self) -> bool {
        self.diskTracker.bytes_consumed() > 0
            || self
                .spillHelper
                .as_ref()
                .is_some_and(topNSpillHelper::isSpillTriggered)
    }
    /// 返回内存追踪器。
    pub fn GetMemTracker(&self) -> &Arc<MemoryTracker> {
        &self.memTracker
    }
    /// 返回磁盘 spill 追踪器。
    pub fn GetDiskTracker(&self) -> &Arc<MemoryTracker> {
        &self.diskTracker
    }
    /// 并行 worker 数量。
    pub fn WorkerConcurrency(&self) -> usize {
        self.concurrency
    }
    /// 查询是否已被中断（kill）。
    pub fn IsKilled(&self) -> bool {
        self.killed.load(std::sync::atomic::Ordering::Acquire)
    }
}
