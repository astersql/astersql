// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 表突变路径上的可复用行编码/校验缓冲。
//
// 提供 `EncodeRowBuffer`（编码后写入 MemBuffer）、`CheckRowBuffer`（约束检查用行视图）
// 以及聚合二者的 `MutateBuffers`，并共享会话级 `WriteStmtBufs` 以降低分配。

use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use crate::{chunk, errctx, errors, intest, kv, rowcodec, tablecodec, time, types, variable};

use super::RowEncodingConfig;

/// 编码单行所需的列 ID 与 Datum，并挂接写语句共享缓冲。
pub struct EncodeRowBuffer {
    /// 待编码列 ID 列表。
    pub colIDs: Vec<i64>,
    /// 与 colIDs 对齐的列值（Datum）。
    pub row: Vec<types::Datum>,
    /// 会话写语句缓冲（含 RowValBuf 等），供编码复用。
    pub writeStmtBufs: Rc<RefCell<variable::WriteStmtBufs>>,
}

impl EncodeRowBuffer {
    /// 清空列数据并按 capacity 预留空间（不缩小已有容量）。
    pub fn Reset(&mut self, capacity: usize) {
        self.colIDs = ensureCapacityAndReset(std::mem::take(&mut self.colIDs), 0, &[capacity]);
        self.row = ensureCapacityAndReset(std::mem::take(&mut self.row), 0, &[capacity]);
    }

    /// 追加一列的 ID 与值。
    pub fn AddColVal(&mut self, colID: i64, val: types::Datum) {
        self.colIDs.push(colID);
        self.row.push(val);
    }

    /// 将当前行编码后写入 MemBuffer；可选行级校验和与写入标志。
    pub fn WriteMemBufferEncoded(
        &mut self,
        cfg: RowEncodingConfig,
        loc: Option<time::Location>,
        ec: errctx::Context,
        memBuffer: &mut dyn kv::MemBuffer,
        key: kv::Key,
        handle: Box<dyn kv::Handle>,
        flags: &[kv::FlagsOp],
    ) -> Result<(), errors::SharedError> {
        // 行级校验和（row-level checksum）开启时，用 handle 构造 RawChecksum。
        let checksum = cfg.IsRowLevelChecksumEnabled.then(|| {
            Box::new(rowcodec::RawChecksum { Handle: handle }) as Box<dyn rowcodec::Checksum>
        });

        let mut stmtBufs = self.writeStmtBufs.borrow_mut();

        // The integrated WriteStmtBufs currently stores the old-row scratch
        // entries as strings. Keep its Go-visible length/capacity contract and
        // let tablecodec allocate its strongly typed Datum scratch vector.
        // 兼容 Go：AddRowValues 以 string 占位，长度约为列数的两倍。
        stmtBufs.AddRowValues.clear();
        stmtBufs
            .AddRowValues
            .resize(self.row.len().saturating_mul(2), String::new());

        let rowEncoder = cfg.RowEncoder.expect("RowEncodingConfig.RowEncoder is nil");
        // 取出并复用会话级 RowValBuf，编码完成后写回。
        let rowValBuf = std::mem::take(&mut stmtBufs.RowValBuf);
        let encoded = tablecodec::EncodeRow(
            loc,
            self.row.clone(),
            self.colIDs.clone(),
            rowValBuf,
            None,
            checksum,
            rowEncoder,
        )
        .map_err(|error| handleEncodingError(&ec, error.to_string()))?;
        stmtBufs.RowValBuf = encoded.clone();
        drop(stmtBufs);

        // 无标志走普通 Set，否则带 PresumeKeyNotExists 等 FlagsOp。
        if flags.is_empty() {
            memBuffer.Set(key, encoded)
        } else {
            memBuffer.SetWithFlags(key, encoded, flags)
        }
    }

    /// 以旧行格式编码 binlog 行数据（不写入 MemBuffer）。
    pub fn EncodeBinlogRowData(
        &self,
        loc: Option<time::Location>,
        ec: errctx::Context,
    ) -> Result<Vec<u8>, errors::SharedError> {
        tablecodec::EncodeOldRow(loc, self.row.clone(), self.colIDs.clone(), Vec::new(), None)
            .map_err(|error| handleEncodingError(&ec, error.to_string()))
    }
}

/// 将编码错误交给 errctx 处理（严格模式可能升级为错误）。
fn handleEncodingError(ec: &errctx::Context, message: String) -> errors::SharedError {
    let error = errors::New(message);
    ec.HandleError(Some(error.clone())).unwrap_or(error)
}

/// 约束检查前暂存的待检行列值。
#[derive(Default)]
pub struct CheckRowBuffer {
    /// 待检查行的 Datum 列表。
    pub rowToCheck: Vec<types::Datum>,
}

/// Owns the chunk backing the row view returned by `GetRowToCheck`.
/// 持有 `GetRowToCheck` 返回行视图背后的 chunk。
pub struct CheckedRow {
    inner: chunk::mutrow::MutRow,
}

impl CheckedRow {
    /// 返回列数。
    pub fn Len(&self) -> usize {
        self.inner.Len()
    }

    /// 按列下标读取 Int64。
    pub fn GetInt64(&self, column: usize) -> i64 {
        self.inner.ToRow().GetInt64(column)
    }
}

impl CheckRowBuffer {
    /// 将缓冲中的 Datum 转为可随机访问的行视图。
    pub fn GetRowToCheck(&self) -> CheckedRow {
        CheckedRow {
            inner: chunk::mutrow::MutRowFromDatums(self.rowToCheck.clone()),
        }
    }

    /// 追加一列待检查值。
    pub fn AddColVal(&mut self, val: types::Datum) {
        self.rowToCheck.push(val);
    }

    /// 清空并按 capacity 预留，不缩小已有容量。
    pub fn Reset(&mut self, capacity: usize) {
        self.rowToCheck =
            ensureCapacityAndReset(std::mem::take(&mut self.rowToCheck), 0, &[capacity]);
    }
}

/// 聚合编码缓冲、检查缓冲与共享 WriteStmtBufs。
pub struct MutateBuffers {
    /// 会话写语句共享缓冲。
    pub stmtBufs: Rc<RefCell<variable::WriteStmtBufs>>,
    /// 行编码缓冲。
    pub encodeRow: EncodeRowBuffer,
    /// 行约束检查缓冲。
    pub checkRow: CheckRowBuffer,
}

/// 由会话级 WriteStmtBufs 构造 MutateBuffers，并共享同一 Rc。
pub fn NewMutateBuffers(stmtBufs: variable::WriteStmtBufs) -> MutateBuffers {
    intest::AssertNotNil(Some(&stmtBufs), &[]);
    let stmtBufs = Rc::new(RefCell::new(stmtBufs));
    MutateBuffers {
        stmtBufs: Rc::clone(&stmtBufs),
        encodeRow: EncodeRowBuffer {
            colIDs: Vec::new(),
            row: Vec::new(),
            writeStmtBufs: Rc::clone(&stmtBufs),
        },
        checkRow: CheckRowBuffer::default(),
    }
}

impl MutateBuffers {
    /// 重置编码缓冲到指定容量并返回可变引用。
    pub fn GetEncodeRowBufferWithCap(&mut self, capacity: usize) -> &mut EncodeRowBuffer {
        self.encodeRow.Reset(capacity);
        &mut self.encodeRow
    }

    /// 重置检查缓冲到指定容量并返回可变引用。
    pub fn GetCheckRowBufferWithCap(&mut self, capacity: usize) -> &mut CheckRowBuffer {
        self.checkRow.Reset(capacity);
        &mut self.checkRow
    }

    /// 借用共享 WriteStmtBufs。
    pub fn GetWriteStmtBufs(&self) -> RefMut<'_, variable::WriteStmtBufs> {
        self.stmtBufs.borrow_mut()
    }
}

/// 调整切片长度并保证容量：容量不足则重新分配；否则原地 resize。
///
/// `optCap` 首元素为期望容量，缺省则等于 `size`；要求 capacity >= size。
pub fn ensureCapacityAndReset<T: Default>(
    mut slice: Vec<T>,
    size: usize,
    optCap: &[usize],
) -> Vec<T> {
    let capacity = optCap.first().copied().unwrap_or(size);
    if slice.capacity() < capacity {
        // Go's make([]T, size, capacity) panics when capacity is smaller
        // than size, but only when this allocation branch is reached.
        assert!(capacity >= size, "capacity must be at least size");
        let mut result = Vec::with_capacity(capacity);
        result.resize_with(size, T::default);
        return result;
    }
    // Go's slice[:size] panics instead of allocating when the existing
    // backing array cannot accommodate size.
    assert!(
        slice.capacity() >= size,
        "size exceeds the existing capacity"
    );
    slice.resize_with(size, T::default);
    slice
}
