// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// CTE 中间结果临时存储（`Storage` / `StorageRC`）。
//
// CTE（公共表表达式）执行时需多次读取同一中间结果：写侧 `Lock` 填入 Chunk、
// `SetDone` 后多读者按引用计数访问；支持内存追踪与溢出（spill）到磁盘。

#![allow(dead_code, non_snake_case, non_camel_case_types)]

use std::any::Any;
use std::sync::Arc;

use parking_lot::Condvar;

use crate::{chunk, disk, errors, memory, syncutil, types};

/// 编译期断言：`Storage` 对象安全约束的占位。
fn _assert_storage_rc<T: Storage>() {}

/// Storage is temporary storage for the intermediate data of a CTE.
///
/// Callers fill the storage once under `Lock`/`Unlock`, mark it done, and then
/// let any number of readers access the immutable chunks.
/// CTE 中间数据的临时存储接口：写侧加锁填充并标记完成，之后多读者只读访问。
pub trait Storage: Any {
    /// Open the underlying storage on the first call, then increment its ref count.
    /// 首次调用打开底层容器并置引用计数为 1，之后每次递增。
    fn OpenAndRef(&mut self) -> Result<(), errors::Error>;

    /// Decrement the ref count and close the underlying storage when it reaches zero.
    /// 递减引用计数；归零时关闭并释放底层容器。
    fn DerefAndClose(&mut self) -> Result<(), errors::Error>;

    /// Swap only the data and schema of two storages, leaving metadata untouched.
    /// 仅交换数据与 schema，保留 done/iter/error 等元数据。
    fn SwapData(&mut self, other: &mut dyn Storage) -> Result<(), errors::Error>;

    /// Reset the storage so it can be filled again.
    /// 重建容器并清空可变状态，以便再次填充。
    fn Reopen(&mut self) -> Result<(), errors::Error>;

    /// Add a chunk; empty chunks are ignored.
    /// 追加 Chunk；空 Chunk 直接忽略。
    fn Add(&mut self, chk: &chunk::Chunk) -> Result<(), errors::Error>;

    /// 按 Chunk 下标取副本。
    fn GetChunk(&self, chkIdx: usize) -> Result<chunk::Chunk, errors::Error>;
    /// 按行指针取行视图。
    fn GetRow(&self, ptr: chunk::RowPtr) -> Result<chunk::Row, errors::Error>;
    /// 已存储的 Chunk 个数。
    fn NumChunks(&self) -> usize;
    /// 已存储的总行数。
    fn NumRows(&self) -> usize;

    /// Storage is not internally thread-safe for mutation. These two methods
    /// preserve Go's explicit cross-call mutex contract.
    /// 变异非内部线程安全；Lock/Unlock 对齐 Go 跨调用显式互斥约定。
    fn Lock(&self);
    fn Unlock(&self);

    /// 写侧是否已标记完成。
    fn Done(&self) -> bool;
    /// 标记写侧完成。
    fn SetDone(&mut self);

    /// 取存储层错误（若有）。
    fn Error(&self) -> Option<&errors::Error>;
    /// 设置存储层错误。
    fn SetError(&mut self, err: errors::Error);

    /// 设置迭代游标（消费者进度元数据）。
    fn SetIter(&mut self, iter: isize);
    /// 读取迭代游标。
    fn GetIter(&self) -> isize;

    /// 内存用量追踪器。
    fn GetMemTracker(&self) -> Arc<memory::Tracker>;
    /// 磁盘用量追踪器。
    fn GetDiskTracker(&self) -> Arc<disk::Tracker>;
    /// 触发溢出落盘的动作对象。
    fn ActionSpill(&self) -> chunk::SpillDiskAction;

    /// 当前占用的内存字节数。
    fn GetMemBytes(&self) -> i64;
    /// 当前占用的磁盘字节数。
    fn GetDiskBytes(&self) -> i64;

    /// 向下转型用：交换数据时识别具体实现类型。
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// A mutex whose guard deliberately survives the `Lock` call until `Unlock`.
/// This is the Rust equivalent of the two-method Go `sync.Mutex` API.
/// 守卫刻意跨过 `Lock` 调用存活到 `Unlock`，模拟 Go `sync.Mutex` 双方法 API。
#[derive(Default)]
struct StorageMutex {
    locked: syncutil::Mutex<bool>,
    available: Condvar,
}

impl StorageMutex {
    /// 阻塞直到获得锁。
    fn lock(&self) {
        let mut locked = self.locked.lock();
        while *locked {
            self.available.wait(&mut locked);
        }
        *locked = true;
    }

    /// 释放锁并唤醒一个等待者。
    fn unlock(&self) {
        let mut locked = self.locked.lock();
        assert!(*locked, "unlock of unlocked CTE storage");
        *locked = false;
        self.available.notify_one();
    }
}

/// StorageRC implements Storage using RowContainer.
/// 基于 `RowContainer` 的 `Storage` 实现。
pub struct StorageRC {
    err: Option<errors::Error>,
    rc: Option<chunk::RowContainer>,
    tp: Vec<types::FieldType>,
    refCnt: isize,
    chkSize: usize,
    iter: isize,
    mu: StorageMutex,
    done: bool,
}

/// Create a new, unopened StorageRC.
/// 创建尚未打开的 `StorageRC`。
pub fn NewStorageRowContainer(tp: Vec<types::FieldType>, chkSize: usize) -> StorageRC {
    StorageRC {
        err: None,
        rc: None,
        tp,
        refCnt: 0,
        chkSize,
        iter: 0,
        mu: StorageMutex::default(),
        done: false,
    }
}

impl Storage for StorageRC {
    fn OpenAndRef(&mut self) -> Result<(), errors::Error> {
        if !self.valid() {
            // 首次打开：创建 RowContainer 并置 refCnt=1。
            self.rc = Some(chunk::RowContainer::New(self.tp.clone(), self.chkSize));
            self.refCnt = 1;
            self.iter = 0;
        } else {
            self.refCnt += 1;
        }
        Ok(())
    }

    fn DerefAndClose(&mut self) -> Result<(), errors::Error> {
        if !self.valid() {
            return Err(errors::New("Storage not opend yet"));
        }

        self.refCnt -= 1;
        if self.refCnt < 0 {
            return Err(errors::New("Storage ref count is less than zero"));
        }
        if self.refCnt == 0 {
            // 归零：关闭容器并复位 done/err/iter；refCnt 置 -1 表示已关闭。
            self.refCnt = -1;
            self.done = false;
            self.err = None;
            self.iter = 0;
            self.rc
                .as_ref()
                .expect("valid storage must have a RowContainer")
                .Close()?;
            self.rc = None;
        }
        Ok(())
    }

    fn SwapData(&mut self, other: &mut dyn Storage) -> Result<(), errors::Error> {
        let Some(otherRC) = other.as_any_mut().downcast_mut::<StorageRC>() else {
            return Err(errors::New(
                "cannot swap if underlying storages are different",
            ));
        };

        // 只交换 schema 与底层容器，不动 refCnt/done/iter/err。
        std::mem::swap(&mut self.tp, &mut otherRC.tp);
        std::mem::swap(&mut self.chkSize, &mut otherRC.chkSize);
        std::mem::swap(&mut self.rc, &mut otherRC.rc);
        Ok(())
    }

    fn Reopen(&mut self) -> Result<(), errors::Error> {
        let Some(rc) = self.rc.as_ref() else {
            return Err(errors::New("Storage is not valid"));
        };
        rc.Close()?;

        self.iter = 0;
        self.done = false;
        self.err = None;
        // RowContainer has tracker and spill-action metadata that Reset does not
        // fully replace, so match Go by constructing a fresh container.
        // Reset 无法完全替换 tracker/spill 元数据，故与 Go 一样新建容器。
        self.rc = Some(chunk::RowContainer::New(self.tp.clone(), self.chkSize));
        Ok(())
    }

    fn Add(&mut self, chk: &chunk::Chunk) -> Result<(), errors::Error> {
        if !self.valid() {
            return Err(errors::New("Storage is not valid"));
        }
        if chk.NumRows() == 0 {
            return Ok(());
        }

        self.rc
            .as_ref()
            .expect("valid storage must have a RowContainer")
            .Add(chk.clone())?;
        Ok(())
    }

    fn GetChunk(&self, chkIdx: usize) -> Result<chunk::Chunk, errors::Error> {
        if !self.valid() {
            return Err(errors::New("Storage is not valid"));
        }
        Ok(self
            .rc
            .as_ref()
            .expect("valid storage must have a RowContainer")
            .GetChunk(chkIdx)?)
    }

    fn GetRow(&self, ptr: chunk::RowPtr) -> Result<chunk::Row, errors::Error> {
        if !self.valid() {
            return Err(errors::New("Storage is not valid"));
        }
        Ok(self
            .rc
            .as_ref()
            .expect("valid storage must have a RowContainer")
            .GetRow(ptr)?)
    }

    fn NumChunks(&self) -> usize {
        self.row_container().NumChunks()
    }

    fn NumRows(&self) -> usize {
        self.row_container().NumRow()
    }

    fn Lock(&self) {
        self.mu.lock();
    }

    fn Unlock(&self) {
        self.mu.unlock();
    }

    fn Done(&self) -> bool {
        self.done
    }

    fn SetDone(&mut self) {
        self.done = true;
    }

    fn Error(&self) -> Option<&errors::Error> {
        self.err.as_ref()
    }

    fn SetError(&mut self, err: errors::Error) {
        self.err = Some(err);
    }

    fn SetIter(&mut self, iter: isize) {
        self.iter = iter;
    }

    fn GetIter(&self) -> isize {
        self.iter
    }

    fn GetMemTracker(&self) -> Arc<memory::Tracker> {
        self.row_container().GetMemTracker()
    }

    fn GetDiskTracker(&self) -> Arc<disk::Tracker> {
        self.row_container().GetDiskTracker()
    }

    fn ActionSpill(&self) -> chunk::SpillDiskAction {
        self.row_container().ActionSpill()
    }

    fn GetMemBytes(&self) -> i64 {
        self.GetMemTracker().BytesConsumed()
    }

    fn GetDiskBytes(&self) -> i64 {
        self.GetDiskTracker().BytesConsumed()
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl StorageRC {
    /// ActionSpillForTest exposes the same action used by the Go spill test.
    /// 暴露与 Go spill 测试相同的溢出动作（含 WaitForTest）。
    pub fn ActionSpillForTest(&self) -> chunk::SpillDiskAction {
        self.row_container().ActionSpillForTest()
    }

    /// 引用计数为正且已持有 RowContainer 时视为有效。
    fn valid(&self) -> bool {
        self.refCnt > 0 && self.rc.is_some()
    }

    /// 取得底层 RowContainer；无效时 panic。
    fn row_container(&self) -> &chunk::RowContainer {
        self.rc.as_ref().expect("Storage is not valid")
    }
}
