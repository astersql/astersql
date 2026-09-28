// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 排序 spill（落盘）动作：在内存超限时触发分区或并行排序的磁盘写出。
//
// 实现 TiDB 风格的 ActionOnExceed 链：本层动作失败或无需执行时可回退到
// `fallback`（例如根 session tracker 的告警/日志动作）。

use crate::parallel_sort_spill_helper::parallelSortSpillHelper;
use crate::sort_partition::SortPartition;
use crate::sort_util::{MemoryTracker, Result, SortError, spillTriggered};
use std::sync::{Arc, Mutex};

/// 内存超限时的可执行动作接口（对应 Go ActionOnExceed）。
pub trait SpillAction: Send + Sync {
    /// 执行 spill 或回退动作。
    fn Action(&self) -> Result<()>;
}

/// 针对单个 [`SortPartition`] 的落盘动作。
pub struct sortPartitionSpillDiskAction {
    partition: Arc<Mutex<SortPartition>>,
    fallback: Option<Arc<dyn SpillAction>>,
}

impl sortPartitionSpillDiskAction {
    /// 构造分区落盘动作；可选 fallback 在已 spill 后继续链式触发。
    pub fn new(
        partition: Arc<Mutex<SortPartition>>,
        fallback: Option<Arc<dyn SpillAction>>,
    ) -> Self {
        Self {
            partition,
            fallback,
        }
    }
    /// 若分区尚未 spill 则写入磁盘；否则执行 fallback（若有）。
    pub fn executeAction(&self) -> Result<()> {
        let mut p = self
            .partition
            .lock()
            .map_err(|_| SortError("sort partition lock poisoned".into()))?;
        if p.spillStatus() != spillTriggered {
            p.spillToDisk()
        } else if let Some(action) = &self.fallback {
            action.Action()
        } else {
            Ok(())
        }
    }
}
impl SpillAction for sortPartitionSpillDiskAction {
    fn Action(&self) -> Result<()> {
        self.executeAction()
    }
}

/// 针对并行排序 spill helper 的落盘动作。
pub struct parallelSortSpillAction {
    helper: Arc<Mutex<parallelSortSpillHelper>>,
    tracker: Arc<MemoryTracker>,
    fallback: Option<Arc<dyn SpillAction>>,
}
impl parallelSortSpillAction {
    /// 构造并行 spill 动作，绑定 helper、内存追踪器与可选 fallback。
    pub fn new(
        helper: Arc<Mutex<parallelSortSpillHelper>>,
        tracker: Arc<MemoryTracker>,
        fallback: Option<Arc<dyn SpillAction>>,
    ) -> Self {
        Self {
            helper,
            tracker,
            fallback,
        }
    }
    /// 仅在传入 tracker 超限时行动，并用 sort tracker 判断是否有足够数据可 spill。
    pub fn executeAction(&self) -> Result<()> {
        if !self.tracker.exceeded() {
            return Ok(());
        }
        let limit = self.tracker.bytes_limit();
        let mut helper = self
            .helper
            .lock()
            .map_err(|_| SortError("parallel spill helper lock poisoned".into()))?;
        let has_enough_data = helper.memoryTracker().bytes_consumed() >= limit / 10;
        if has_enough_data {
            if helper.setNeedSpill() {
                helper.spill()?;
            }
            return Ok(());
        }
        if let Some(action) = &self.fallback {
            action.Action()?;
        }
        Ok(())
    }
}
impl SpillAction for parallelSortSpillAction {
    fn Action(&self) -> Result<()> {
        self.executeAction()
    }
}
