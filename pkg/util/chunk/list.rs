// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 多 Chunk 列表：按配置的块大小追加行，并用内存 Tracker 记账。
//
// 对应 Go `pkg/util/chunk/list.go`。`List` 用 `Vec<Box<Chunk>>` 保持 Chunk 地址稳定
//（`Row` 存指向 Chunk 的指针，对齐 Go 的 `[]*Chunk`）。`RowPtr` 以 (chunk 下标, 行下标)
// 定位行；`Reset` 把 Chunk 回收到 freelist，`Clear` 释放并清零 Tracker。

use crate::{Chunk, ChunkError, New, Renew, Row, memory, types};

/// Holds chunks and appends rows while respecting the configured chunk size.
/// 持有多个 Chunk 并按 `maxChunkSize` 追加行；`consumedIdx` 标记已计入 Tracker 的最后一块。
// Box keeps Chunk addresses stable when the outer Vec reallocates; Row stores a
// pointer to its Chunk, matching Go's []*Chunk representation.
#[allow(clippy::vec_box)]
pub struct List {
    fieldTypes: Vec<types::FieldType>,
    initChunkSize: usize,
    maxChunkSize: usize,
    length: usize,
    chunks: Vec<Box<Chunk>>,
    freelist: Vec<Box<Chunk>>,
    memTracker: Box<memory::Tracker>,
    consumedIdx: isize,
}

/// `RowPtr` 结构体的字节大小，便于内存估算。
pub const RowPtrSize: usize = std::mem::size_of::<RowPtr>();

/// 行在 `List` 中的轻量引用：第 `ChkIdx` 个 Chunk 的第 `RowIdx` 行。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RowPtr {
    pub ChkIdx: u32,
    pub RowIdx: u32,
}

/// 使用外部注入的内存 Tracker 构造空 `List`。
pub fn NewListWithMemTracker(
    fieldTypes: Vec<types::FieldType>,
    initChunkSize: usize,
    maxChunkSize: usize,
    tracker: Box<memory::Tracker>,
) -> Box<List> {
    Box::new(List {
        fieldTypes,
        initChunkSize,
        maxChunkSize,
        length: 0,
        chunks: Vec::new(),
        freelist: Vec::new(),
        memTracker: tracker,
        consumedIdx: -1,
    })
}

/// 使用默认 `LabelForChunkList` Tracker（无上限）构造 `List`。
pub fn NewList(
    fieldTypes: Vec<types::FieldType>,
    initChunkSize: usize,
    maxChunkSize: usize,
) -> Box<List> {
    NewListWithMemTracker(
        fieldTypes,
        initChunkSize,
        maxChunkSize,
        memory::NewTracker(memory::LabelForChunkList, -1),
    )
}

impl List {
    /// 等价于 [`NewList`]。
    pub fn New(
        fieldTypes: Vec<types::FieldType>,
        initChunkSize: usize,
        maxChunkSize: usize,
    ) -> Box<List> {
        NewList(fieldTypes, initChunkSize, maxChunkSize)
    }

    /// 返回内存 Tracker 的不可变引用。
    pub fn GetMemTracker(&self) -> &memory::Tracker {
        &self.memTracker
    }

    /// 返回内存 Tracker 的可变引用。
    pub fn GetMemTrackerMut(&mut self) -> &mut memory::Tracker {
        &mut self.memTracker
    }

    /// 列表中的总行数。
    pub fn Len(&self) -> usize {
        self.length
    }

    /// 当前持有的 Chunk 个数。
    pub fn NumChunks(&self) -> usize {
        self.chunks.len()
    }

    /// 各列的字段类型。
    pub fn FieldTypes(&self) -> &[types::FieldType] {
        &self.fieldTypes
    }

    /// 指定 Chunk 内的行数。
    pub fn NumRowsOfChunk(&self, chkID: usize) -> usize {
        self.chunks[chkID].NumRows()
    }

    /// 按索引取 Chunk 引用。
    pub fn GetChunk(&self, chkIdx: usize) -> &Chunk {
        &self.chunks[chkIdx]
    }

    /// 追加一行；必要时分配新 Chunk，并返回该行的 `RowPtr`。
    pub fn AppendRow(&mut self, row: Row) -> RowPtr {
        let mut chkIdx = self.chunks.len() as isize - 1;
        // 无块、已满，或末块已计入 Tracker（视为“只读”）时都需要新块。
        let needs_chunk = chkIdx == -1
            || self.chunks[chkIdx as usize].NumRows() >= self.chunks[chkIdx as usize].Capacity()
            || chkIdx == self.consumedIdx;
        if needs_chunk {
            let new_chunk = self.AllocChunk();
            self.chunks.push(new_chunk);
            if chkIdx != self.consumedIdx {
                self.memTracker
                    .Consume(self.chunks[chkIdx as usize].MemoryUsage());
                self.consumedIdx = chkIdx;
            }
            chkIdx += 1;
        }

        let chunk = &mut self.chunks[chkIdx as usize];
        let rowIdx = chunk.NumRows();
        chunk.AppendRow(row);
        self.length += 1;
        RowPtr {
            ChkIdx: chkIdx as u32,
            RowIdx: rowIdx as u32,
        }
    }

    /// Takes ownership of a non-empty chunk, as the Go implementation does.
    /// 接管非空 Chunk 的所有权并计入 Tracker（对齐 Go `List.Add`）。
    pub fn Add(&mut self, chunk: Box<Chunk>) {
        assert!(
            chunk.NumRows() > 0,
            "chunk appended to List should have at least 1 row"
        );
        let chkIdx = self.chunks.len() as isize - 1;
        if self.consumedIdx != chkIdx {
            self.memTracker
                .Consume(self.chunks[chkIdx as usize].MemoryUsage());
            self.consumedIdx = chkIdx;
        }
        self.memTracker.Consume(chunk.MemoryUsage());
        self.consumedIdx += 1;
        self.length += chunk.NumRows();
        self.chunks.push(chunk);
    }

    /// 从 freelist 复用或新建/续建 Chunk，供追加使用。
    pub fn AllocChunk(&mut self) -> Box<Chunk> {
        if let Some(mut chunk) = self.freelist.pop() {
            // 复用前先从 Tracker 扣回旧用量，Reset 后再由调用方重新记账。
            self.memTracker.Consume(-chunk.MemoryUsage());
            chunk.Reset();
            return chunk;
        }
        if let Some(last) = self.chunks.last() {
            return Renew(last, self.maxChunkSize);
        }
        New(
            self.fieldTypes.clone(),
            self.initChunkSize,
            self.maxChunkSize,
        )
    }

    /// 按 `RowPtr` 解析出行视图。
    pub fn GetRow(&self, ptr: RowPtr) -> Row {
        self.chunks[ptr.ChkIdx as usize].GetRow(ptr.RowIdx as usize)
    }

    /// 清空行数并把 Chunk 移入 freelist 以便复用；保留已记账内存直至再次追加。
    pub fn Reset(&mut self) {
        let lastIdx = self.chunks.len() as isize - 1;
        if lastIdx != self.consumedIdx {
            self.memTracker
                .Consume(self.chunks[lastIdx as usize].MemoryUsage());
        }
        self.freelist.append(&mut self.chunks);
        self.length = 0;
        self.consumedIdx = -1;
    }

    /// 释放全部 Chunk 与 freelist，并将 Tracker 用量归零。
    pub fn Clear(&mut self) {
        self.memTracker.Consume(-self.memTracker.BytesConsumed());
        self.freelist.clear();
        self.chunks.clear();
        self.length = 0;
        self.consumedIdx = -1;
    }

    /// 按 Chunk、行顺序回调 `walkFunc`；回调返回错误则中止。
    pub fn Walk(&self, mut walkFunc: ListWalkFunc) -> Result<(), ChunkError> {
        for chunk in &self.chunks {
            for rowIdx in 0..chunk.NumRows() {
                walkFunc(chunk.GetRow(rowIdx))?;
            }
        }
        Ok(())
    }
}

/// 遍历 `List` 时每行回调；返回 `Err` 可提前终止。
pub type ListWalkFunc = Box<dyn FnMut(Row) -> Result<(), ChunkError>>;
