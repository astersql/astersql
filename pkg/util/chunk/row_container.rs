// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 行容器：内存 List 存 Chunk，内存压力时 spill（落盘）到按行组织的磁盘结构。
//
// 对应 Go `row_container.go`。`RowContainer` 在 spill 后原子切换读写到磁盘；
// `SortedRowContainer` 在排序后禁止再追加。`SpillDiskAction` 对接内存 Tracker
// 的回调，在超限时异步触发 spill。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;

use crate::{
    Chunk, ChunkError, CompareFunc, DataInDiskByRows, List, Result, Row, RowPtr, disk, memory,
    types,
};

/// 排序完成后禁止再 Add 的错误消息。
pub const ErrCannotAddBecauseSorted: &str = "can not add because sorted";
/// 排序比较次数检查点：累计达到该值时向 memTracker 上报一次信号。
pub const SignalCheckpointForSort: u32 = 10_240;

/// 容器内部记录：内存 List、可选磁盘、以及 spill 过程错误。
struct rowContainerRecord {
    inMemory: List,
    inDisk: Option<DataInDiskByRows>,
    spillError: Option<String>,
}

/// RowContainer keeps chunks in memory until spill, then atomically switches
/// every read and subsequent append to the row-oriented disk representation.
///
/// 内存中持有 Chunk，spill 后原子切换到按行磁盘表示，后续读写均走磁盘。
#[derive(Clone)]
pub struct RowContainer {
    /// 受 RwLock 保护的内存/磁盘记录。
    records: Arc<RwLock<rowContainerRecord>>,
    /// 内存用量 Tracker。
    memTracker: Arc<memory::Tracker>,
    /// 磁盘用量 Tracker。
    diskTracker: Arc<memory::Tracker>,
    /// 可选的 spill 动作句柄。
    actionSpill: Arc<Mutex<Option<SpillDiskAction>>>,
}

impl RowContainer {
    /// 按字段类型与 Chunk 大小创建空容器，并挂接内存 Tracker。
    pub fn New(fieldTypes: Vec<types::FieldType>, chunkSize: usize) -> Self {
        let mut inMemory = List::New(fieldTypes, chunkSize, chunkSize);
        let memTracker = Arc::from(memory::NewTracker(memory::LabelForRowContainer, -1));
        inMemory
            .GetMemTrackerMut()
            .AttachTo(Arc::as_ptr(&memTracker) as *mut memory::Tracker);
        Self {
            records: Arc::new(RwLock::new(rowContainerRecord {
                inMemory: *inMemory,
                inDisk: None,
                spillError: None,
            })),
            memTracker,
            diskTracker: Arc::from(disk::NewTracker(memory::LabelForRowContainer, -1)),
            actionSpill: Arc::new(Mutex::new(None)),
        }
    }

    /// Rust's cloned Arc supplies the Go shallow-copy semantics: data and
    /// trackers are shared while each caller owns an independent handle.
    ///
    /// Arc 克隆提供与 Go 浅拷贝相同语义：数据与 Tracker 共享，句柄独立。
    pub fn ShallowCopyWithNewMutex(&self) -> Self {
        self.clone()
    }

    /// 将内存数据 spill 到磁盘（无预置错误）。
    pub fn SpillToDisk(&self) {
        self.spillToDisk(None);
    }
    /// 是否有足够数据值得 spill；当前恒为 true（与 Go 测试桩对齐）。
    pub fn hasEnoughDataToSpill(&self, _tracker: &memory::Tracker) -> bool {
        true
    }

    /// 执行 spill：把内存各 Chunk 写入 `DataInDiskByRows`，捕获 panic/错误后清空内存。
    fn spillToDisk(&self, preSpillError: Option<ChunkError>) {
        let mut records = self.records.write().unwrap();
        if records.inDisk.is_some() {
            return;
        }

        if let Some(action) = self.actionSpill.lock().unwrap().as_ref() {
            if action.getStatus() == spillStatus::spilledYet {
                return;
            }
            action.setStatus(spillStatus::spilling);
        }
        memory::QueryForceDisk.Add(1);

        let mut inDisk = DataInDiskByRows::New(records.inMemory.FieldTypes().to_vec());
        if let Some(error) = preSpillError {
            // 排序等前置失败时直接记录错误，仍切换到磁盘句柄。
            records.spillError = Some(error.to_string());
        } else {
            let spillResult = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if failpointBool("spillToDiskOutOfDiskQuota") {
                    panic!("out of disk quota when spilling")
                }
                for chunkIdx in 0..records.inMemory.NumChunks() {
                    inDisk.Add(&records.inMemory.GetChunk(chunkIdx))?;
                    records.inMemory.GetMemTracker().HandleKillSignal();
                }
                Result::<()>::Ok(())
            }));
            match spillResult {
                Ok(Ok(())) => records.inMemory.Clear(),
                Ok(Err(error)) => records.spillError = Some(error.to_string()),
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("panic while spilling");
                    records.spillError = Some(message.to_owned());
                }
            }
        }
        self.diskTracker
            .Consume(inDisk.GetDiskTracker().BytesConsumed());
        records.inDisk = Some(inDisk);
        drop(records);

        if let Some(action) = self.actionSpill.lock().unwrap().as_ref() {
            action.setStatus(spillStatus::spilledYet);
        }
    }

    /// 重置：关闭磁盘或 Reset 内存 List，并清除 spill 错误。
    pub fn Reset(&self) -> Result<()> {
        let mut records = self.records.write().unwrap();
        if let Some(mut disk) = records.inDisk.take() {
            disk.Close()?;
            self.diskTracker.Consume(-self.diskTracker.BytesConsumed());
            if let Some(action) = self.actionSpill.lock().unwrap().as_ref() {
                action.Reset();
            }
        } else {
            records.inMemory.Reset();
        }
        records.spillError = None;
        Ok(())
    }

    /// 是否已落盘。
    fn alreadySpilled(&self) -> bool {
        self.records.read().unwrap().inDisk.is_some()
    }
    /// 测试用：是否已 spill。
    pub fn AlreadySpilledSafeForTest(&self) -> bool {
        self.alreadySpilled()
    }

    /// 总行数（磁盘或内存）。
    pub fn NumRow(&self) -> usize {
        let records = self.records.read().unwrap();
        records
            .inDisk
            .as_ref()
            .map_or_else(|| records.inMemory.Len(), DataInDiskByRows::Len)
    }

    /// 指定 Chunk 的行数。
    pub fn NumRowsOfChunk(&self, chunkID: usize) -> usize {
        let records = self.records.read().unwrap();
        records.inDisk.as_ref().map_or_else(
            || records.inMemory.NumRowsOfChunk(chunkID),
            |disk| disk.NumRowsOfChunk(chunkID),
        )
    }

    /// Chunk 数量。
    pub fn NumChunks(&self) -> usize {
        let records = self.records.read().unwrap();
        records
            .inDisk
            .as_ref()
            .map_or_else(|| records.inMemory.NumChunks(), DataInDiskByRows::NumChunks)
    }

    /// 追加一个 Chunk；若有 spill 错误则直接失败。
    pub fn Add(&self, chunk: Chunk) -> Result<()> {
        if failpointBool("testRowContainerDeadLock") {
            thread::sleep(std::time::Duration::from_secs(1));
        }
        let mut records = self.records.write().unwrap();
        if let Some(error) = records.spillError.clone() {
            return Err(ChunkError::Message(error));
        }
        if let Some(disk) = records.inDisk.as_mut() {
            let before = disk.GetDiskTracker().BytesConsumed();
            disk.Add(&chunk)?;
            self.diskTracker
                .Consume(disk.GetDiskTracker().BytesConsumed() - before);
        } else {
            records.inMemory.Add(Box::new(chunk));
        }
        Ok(())
    }

    /// 从内存 List 分配新 Chunk 骨架。
    pub fn AllocChunk(&self) -> Chunk {
        *self.records.write().unwrap().inMemory.AllocChunk()
    }

    /// 按块下标取 Chunk（磁盘路径会物化；内存路径深拷贝）。
    pub fn GetChunk(&self, chunkIdx: usize) -> Result<Chunk> {
        let mut records = self.records.write().unwrap();
        if let Some(error) = records.spillError.clone() {
            return Err(ChunkError::Message(error));
        }
        match records.inDisk.as_mut() {
            Some(disk) => disk.GetChunk(chunkIdx),
            None => Ok(*records.inMemory.GetChunk(chunkIdx).CopyConstruct()),
        }
    }

    /// 按 `RowPtr`（块下标+行下标）取行。
    pub fn GetRow(&self, ptr: RowPtr) -> Result<Row> {
        let mut records = self.records.write().unwrap();
        if let Some(error) = records.spillError.clone() {
            return Err(ChunkError::Message(error));
        }
        match records.inDisk.as_mut() {
            Some(disk) => disk.GetRow(ptr),
            None => Ok(records.inMemory.GetRow(ptr)),
        }
    }

    /// 磁盘路径下取行并追加到可选 Chunk；内存路径不追加。
    pub fn GetRowAndAppendToChunkIfInDisk(
        &self,
        ptr: RowPtr,
        chunk: Option<Chunk>,
    ) -> Result<(Row, Option<Chunk>)> {
        let mut records = self.records.write().unwrap();
        if let Some(error) = records.spillError.clone() {
            return Err(ChunkError::Message(error));
        }
        if let Some(disk) = records.inDisk.as_mut() {
            let (row, chunk) = disk.GetRowAndAppendToChunk(ptr, chunk)?;
            Ok((row, Some(chunk)))
        } else {
            Ok((records.inMemory.GetRow(ptr), None))
        }
    }

    /// 无论是否在磁盘，都将行追加到目标 Chunk 后返回。
    pub fn GetRowAndAlwaysAppendToChunk(
        &self,
        ptr: RowPtr,
        mut chunk: Chunk,
    ) -> Result<(Row, Chunk)> {
        let (row, appended) = self.GetRowAndAppendToChunkIfInDisk(ptr, Some(chunk.clone()))?;
        if appended.is_none() {
            chunk.AppendRow(row.clone());
        }
        Ok((row, appended.unwrap_or(chunk)))
    }

    /// 内存 Tracker。
    pub fn GetMemTracker(&self) -> Arc<memory::Tracker> {
        self.memTracker.clone()
    }
    /// 磁盘 Tracker。
    pub fn GetDiskTracker(&self) -> Arc<memory::Tracker> {
        self.diskTracker.clone()
    }

    /// 关闭容器：标记动作完成、关磁盘、清内存。
    pub fn Close(&self) -> Result<()> {
        if let Some(action) = self.actionSpill.lock().unwrap().as_ref() {
            action.SetFinished();
        }
        let mut records = self.records.write().unwrap();
        if let Some(mut disk) = records.inDisk.take() {
            disk.Close()?;
        }
        records.inMemory.Clear();
        records.spillError = None;
        self.diskTracker.Consume(-self.diskTracker.BytesConsumed());
        Ok(())
    }

    /// 懒创建并返回 spill 动作（对接内存超限回调）。
    pub fn ActionSpill(&self) -> SpillDiskAction {
        let mut action = self.actionSpill.lock().unwrap();
        action
            .get_or_insert_with(|| {
                let target = self.clone();
                SpillDiskAction::new(Arc::new(move || target.SpillToDisk()), Arc::new(|_| true))
            })
            .clone()
    }

    /// 测试入口，等同 `ActionSpill`。
    pub fn ActionSpillForTest(&self) -> SpillDiskAction {
        self.ActionSpill()
    }
}

impl crate::row_container_reader::RowContainerSource for RowContainer {
    fn NumChunks(&self) -> usize {
        RowContainer::NumChunks(self)
    }

    fn NumRowsOfChunk(&self, index: usize) -> usize {
        RowContainer::NumRowsOfChunk(self, index)
    }

    fn RowsOfChunk(&self, index: usize) -> Result<Vec<Row>> {
        (0..self.NumRowsOfChunk(index))
            .map(|row| {
                self.GetRow(RowPtr {
                    ChkIdx: index as u32,
                    RowIdx: row as u32,
                })
            })
            .collect()
    }
}

/// spill 状态机：未 spill / 进行中 / 已完成。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum spillStatus {
    notSpilled,
    spilling,
    spilledYet,
}

/// spill 动作内部状态：状态、触发标志、运行计数与回调。
struct baseSpillDiskAction {
    status: Mutex<spillStatus>,
    statusChanged: Condvar,
    triggered: AtomicBool,
    finished: AtomicBool,
    running: Mutex<usize>,
    runningChanged: Condvar,
    spill: Arc<dyn Fn() + Send + Sync>,
    enough: Arc<dyn Fn(&memory::Tracker) -> bool + Send + Sync>,
    fallback: Mutex<Option<Arc<dyn Fn(&memory::Tracker) + Send + Sync>>>,
}

/// 内存 Tracker 触发的异步 spill 动作句柄。
#[derive(Clone)]
pub struct SpillDiskAction(Arc<baseSpillDiskAction>);

impl SpillDiskAction {
    /// 组装动作：提供 spill 闭包与“数据是否足够”判定。
    fn new(
        spill: Arc<dyn Fn() + Send + Sync>,
        enough: Arc<dyn Fn(&memory::Tracker) -> bool + Send + Sync>,
    ) -> Self {
        Self(Arc::new(baseSpillDiskAction {
            status: Mutex::new(spillStatus::notSpilled),
            statusChanged: Condvar::new(),
            triggered: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            running: Mutex::new(0),
            runningChanged: Condvar::new(),
            spill,
            enough,
            fallback: Mutex::new(None),
        }))
    }

    /// Tracker 回调入口：首次触发时异步 spill；若正在 spill 则等待；仍超限则走 fallback。
    pub fn Action(&self, tracker: Arc<memory::Tracker>) {
        if self.getStatus() == spillStatus::notSpilled
            && (self.0.enough)(&tracker)
            && !self.0.triggered.swap(true, AtomicOrdering::SeqCst)
        {
            *self.0.running.lock().unwrap() += 1;
            let action = self.clone();
            thread::spawn(move || {
                action.setStatus(spillStatus::spilling);
                (action.0.spill)();
                action.setStatus(spillStatus::spilledYet);
                let mut running = action.0.running.lock().unwrap();
                *running -= 1;
                action.0.runningChanged.notify_all();
            });
            return;
        }

        // 等待进行中的 spill 结束。
        let mut status = self.0.status.lock().unwrap();
        while *status == spillStatus::spilling {
            status = self.0.statusChanged.wait(status).unwrap();
        }
        drop(status);
        if tracker.CheckExceed() {
            if let Some(fallback) = self.0.fallback.lock().unwrap().as_ref() {
                fallback(&tracker);
            }
        }
    }

    /// 设置 spill 后仍超限时的兜底回调。
    pub fn SetFallback(&self, fallback: Arc<dyn Fn(&memory::Tracker) + Send + Sync>) {
        *self.0.fallback.lock().unwrap() = Some(fallback);
    }
    /// 更新状态并唤醒等待者。
    pub fn setStatus(&self, status: spillStatus) {
        *self.0.status.lock().unwrap() = status;
        self.0.statusChanged.notify_all();
    }
    /// 读取当前 spill 状态。
    pub fn getStatus(&self) -> spillStatus {
        *self.0.status.lock().unwrap()
    }
    /// 重置触发标志与状态，便于再次 spill。
    pub fn Reset(&self) {
        self.0.triggered.store(false, AtomicOrdering::SeqCst);
        self.0.finished.store(false, AtomicOrdering::SeqCst);
        self.setStatus(spillStatus::notSpilled);
    }
    /// Tracker 动作优先级（固定为 2，对齐 Go）。
    pub fn GetPriority(&self) -> i64 {
        2
    }
    /// 标记动作结束（容器 Close 时调用）。
    pub fn SetFinished(&self) {
        self.0.finished.store(true, AtomicOrdering::SeqCst);
        self.setStatus(spillStatus::spilledYet);
    }
    /// 测试用：等待异步 spill 线程结束。
    pub fn WaitForTest(&self) {
        let mut running = self.0.running.lock().unwrap();
        while *running != 0 {
            running = self.0.runningChanged.wait(running).unwrap();
        }
    }
}

/// 可排序行容器的内部状态。
struct SortedRowContainerInner {
    rowContainer: RowContainer,
    /// 排序后的行指针列表；`Some` 表示已排序且禁止再 Add。
    rowPtrs: RwLock<Option<Vec<RowPtr>>>,
    /// 各排序键是否降序。
    ByItemsDesc: Vec<bool>,
    /// 排序键列下标。
    keyColumns: Vec<usize>,
    /// 各键的比较函数。
    keyCmpFuncs: Vec<CompareFunc>,
    actionSpill: Mutex<Option<SortAndSpillDiskAction>>,
    memTracker: Arc<memory::Tracker>,
    /// 排序比较次数计数器（用于检查点信号）。
    timesOfRowCompare: AtomicU32,
}

/// 带排序与 spill 的行容器包装。
#[derive(Clone)]
pub struct SortedRowContainer(Arc<SortedRowContainerInner>);

impl SortedRowContainer {
    /// 创建可排序容器。
    pub fn New(
        fieldTypes: Vec<types::FieldType>,
        chunkSize: usize,
        ByItemsDesc: Vec<bool>,
        keyColumns: Vec<usize>,
        keyCmpFuncs: Vec<CompareFunc>,
    ) -> Self {
        let mut rowContainer = RowContainer::New(fieldTypes, chunkSize);
        let memTracker = Arc::from(memory::NewTracker(memory::LabelForRowContainer, -1));
        Arc::get_mut(&mut rowContainer.memTracker)
            .expect("new row container owns its memory tracker")
            .AttachTo(Arc::as_ptr(&memTracker) as *mut memory::Tracker);
        Self(Arc::new(SortedRowContainerInner {
            rowContainer,
            rowPtrs: RwLock::new(None),
            ByItemsDesc,
            keyColumns,
            keyCmpFuncs,
            actionSpill: Mutex::new(None),
            memTracker,
            timesOfRowCompare: AtomicU32::new(0),
        }))
    }

    /// 关闭：释放行指针记账并关闭底层容器。
    pub fn Close(&self) -> Result<()> {
        let rowCount = self.NumRow() as i64;
        *self.0.rowPtrs.write().unwrap() = None;
        self.0.memTracker.Consume(-8 * rowCount);
        self.0.rowContainer.Close()
    }

    /// 多键比较：累计比较次数，达检查点时向 Tracker 发信号；支持升/降序。
    fn compareRows(&self, left: &Row, right: &Row) -> Ordering {
        if signalCheckpointForSortInjected() {
            self.0
                .timesOfRowCompare
                .fetch_add(1024, AtomicOrdering::Relaxed);
        }
        let comparisons = self
            .0
            .timesOfRowCompare
            .fetch_add(1, AtomicOrdering::Relaxed)
            + 1;
        if comparisons >= SignalCheckpointForSort {
            self.0.timesOfRowCompare.store(0, AtomicOrdering::Relaxed);
            self.0.memTracker.Consume(1);
        }
        for (idx, column) in self.0.keyColumns.iter().copied().enumerate() {
            let mut ordering =
                (self.0.keyCmpFuncs[idx])(left.clone(), column, right.clone(), column).cmp(&0);
            if self.0.ByItemsDesc[idx] {
                ordering = ordering.reverse();
            }
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    }

    /// 构建全部 `RowPtr` 并按键排序；已排序则幂等返回。
    pub fn Sort(&self) -> Result<()> {
        let mut rowPtrs = self.0.rowPtrs.write().unwrap();
        if rowPtrs.is_some() {
            return Ok(());
        }

        let mut pointers = Vec::with_capacity(self.NumRow());
        for chunkIdx in 0..self.NumChunks() {
            let chunk = self.0.rowContainer.GetChunk(chunkIdx)?;
            for rowIdx in 0..chunk.NumRows() {
                pointers.push(RowPtr {
                    ChkIdx: chunkIdx as u32,
                    RowIdx: rowIdx as u32,
                });
            }
        }
        // Go publishes the pointer slice before sort.Slice. If sorting panics,
        // the recovered error still leaves the container closed to later Add.
        *rowPtrs = Some(pointers);
        let sortResult = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if failpointBool("errorDuringSortRowContainer") {
                panic!("sort meet error")
            }
            rowPtrs.as_mut().unwrap().sort_unstable_by(|left, right| {
                let left = self
                    .0
                    .rowContainer
                    .GetRow(*left)
                    .expect("in-memory row pointer");
                let right = self
                    .0
                    .rowContainer
                    .GetRow(*right)
                    .expect("in-memory row pointer");
                self.compareRows(&left, &right)
            });
        }));

        match sortResult {
            Ok(()) => Ok(()),
            Err(payload) => {
                let message = payload
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("panic while sorting");
                Err(ChunkError::Message(message.to_owned()))
            }
        }
    }

    /// 先 Sort，再把排序错误（若有）作为预置错误传入底层 spill。
    pub fn SpillToDisk(&self) {
        let error = self.Sort().err();
        self.0.rowContainer.spillToDisk(error);
    }

    /// 排序容器：内存占用超过 Tracker 限额 10% 才认为值得 spill。
    pub fn hasEnoughDataToSpill(&self, tracker: &memory::Tracker) -> bool {
        self.0.memTracker.BytesConsumed() > tracker.GetBytesLimit() / 10
    }

    /// 追加 Chunk；已排序则返回 `ErrCannotAddBecauseSorted`。
    pub fn Add(&self, chunk: Chunk) -> Result<()> {
        if self.0.rowPtrs.read().unwrap().is_some() {
            return Err(ChunkError::Message(ErrCannotAddBecauseSorted.to_owned()));
        }
        self.0.memTracker.Consume((chunk.NumRows() * 8) as i64);
        self.0.rowContainer.Add(chunk)
    }

    /// 按排序后的下标取行。
    pub fn GetSortedRow(&self, idx: usize) -> Result<Row> {
        let ptr = self.0.rowPtrs.read().unwrap().as_ref().ok_or_else(|| {
            ChunkError::Message("sorted row pointers are not initialized".to_owned())
        })?[idx];
        self.0.rowContainer.GetRow(ptr)
    }

    /// 取排序行并保证追加到目标 Chunk。
    pub fn GetSortedRowAndAlwaysAppendToChunk(
        &self,
        idx: usize,
        chunk: Chunk,
    ) -> Result<(Row, Chunk)> {
        let ptr = self.0.rowPtrs.read().unwrap().as_ref().ok_or_else(|| {
            ChunkError::Message("sorted row pointers are not initialized".to_owned())
        })?[idx];
        self.0.rowContainer.GetRowAndAlwaysAppendToChunk(ptr, chunk)
    }

    /// 懒创建“先排序再 spill”的动作。
    pub fn ActionSpill(&self) -> SortAndSpillDiskAction {
        let mut action = self.0.actionSpill.lock().unwrap();
        action
            .get_or_insert_with(|| {
                let target = self.clone();
                let enoughTarget = self.clone();
                SortAndSpillDiskAction(SpillDiskAction::new(
                    Arc::new(move || target.SpillToDisk()),
                    Arc::new(move |tracker| enoughTarget.hasEnoughDataToSpill(tracker)),
                ))
            })
            .clone()
    }
    /// 测试入口，等同 `ActionSpill`。
    pub fn ActionSpillForTest(&self) -> SortAndSpillDiskAction {
        self.ActionSpill()
    }
    pub fn GetMemTracker(&self) -> Arc<memory::Tracker> {
        self.0.memTracker.clone()
    }
    pub fn NumRow(&self) -> usize {
        self.0.rowContainer.NumRow()
    }
    pub fn NumChunks(&self) -> usize {
        self.0.rowContainer.NumChunks()
    }
}

/// failpoint：注入排序检查点加速信号。
fn signalCheckpointForSortInjected() -> bool {
    failpointBool("SignalCheckpointForSort")
}

/// 解析 Go failpoint `return(bool)`；未启用、无参数及非真值均视为 false。
fn failpointBool(name: &str) -> bool {
    fail::eval(name, |argument| {
        argument.is_some_and(|argument| matches!(argument.trim(), "1" | "true" | "on"))
    })
    .unwrap_or(false)
}

/// 排序容器专用的 spill 动作包装。
#[derive(Clone)]
pub struct SortAndSpillDiskAction(SpillDiskAction);

impl SortAndSpillDiskAction {
    /// 转发到底层 `SpillDiskAction::Action`。
    pub fn Action(&self, tracker: Arc<memory::Tracker>) {
        self.0.Action(tracker);
    }
    /// 测试用等待。
    pub fn WaitForTest(&self) {
        self.0.WaitForTest();
    }
}
