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

// Chunk / List / Row 容器的行迭代器族。
//
// 对应 Go `pkg/util/chunk/iterator.go`。统一 `Iterator` 接口：`Begin`/`Next`/`Current`/
// `End`/`ReachEnd`/`Len`/`Error`。游标语义对齐 Go：`Begin` 返回首行并把内部游标置于
// “下一行”；`End` 用空 `Row` 作哨兵。`MultiIterator` 串联多个非空迭代器。

use crate::list::{List, RowPtr};
use crate::{Chunk, ChunkError, Row, RowContainer};

/// 行迭代公共接口；`Error` 用于可能失败的磁盘/容器读路径。
pub trait Iterator {
    /// 定位到首行并返回；空集合返回 `End`。
    fn Begin(&mut self) -> Row;
    /// 前进并返回下一行；越界后返回 `End`。
    fn Next(&mut self) -> Row;
    /// 结束哨兵（空行）。
    fn End(&self) -> Row;
    /// 可迭代的总行数。
    fn Len(&self) -> usize;
    /// 当前行（游标尚未 `Begin` 或已越过末尾时返回 `End`）。
    fn Current(&mut self) -> Row;
    /// 强制将游标推到结束位置。
    fn ReachEnd(&mut self);
    /// 最近一次错误（多数内存迭代器恒为 `None`）。
    fn Error(&self) -> Option<ChunkError>;
}

/// 64 字节对齐填充，降低游标与数据字段的伪共享（对齐 Go 侧 cache line pad）。
#[repr(align(64))]
#[derive(Default)]
struct CacheLinePad;

/// 基于已有 `Vec<Row>` 构造切片迭代器。
pub fn NewIterator4Slice(rows: Vec<Row>) -> Box<Iterator4Slice> {
    Box::new(Iterator4Slice {
        _pad0: CacheLinePad,
        rows,
        cursor: 0,
        _pad1: CacheLinePad,
    })
}

/// 对行切片的迭代器；`cursor` 为“下一行”下标（`Begin` 后为 1）。
pub struct Iterator4Slice {
    _pad0: CacheLinePad,
    rows: Vec<Row>,
    cursor: usize,
    _pad1: CacheLinePad,
}

impl Iterator for Iterator4Slice {
    fn Begin(&mut self) -> Row {
        if self.Len() == 0 {
            return self.End();
        }
        self.cursor = 1;
        self.rows[0].clone()
    }

    fn Next(&mut self) -> Row {
        let len = self.Len();
        if self.cursor >= len {
            self.cursor = len + 1;
            return self.End();
        }
        let row = self.rows[self.cursor].clone();
        self.cursor += 1;
        row
    }

    fn Current(&mut self) -> Row {
        if self.cursor == 0 || self.cursor > self.Len() {
            self.End()
        } else {
            self.rows[self.cursor - 1].clone()
        }
    }

    fn End(&self) -> Row {
        Row::default()
    }
    fn ReachEnd(&mut self) {
        self.cursor = self.Len() + 1;
    }
    fn Len(&self) -> usize {
        self.rows.len()
    }
    fn Error(&self) -> Option<ChunkError> {
        None
    }
}

impl Iterator4Slice {
    /// 换一批行并重置游标。
    pub fn Reset(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.cursor = 0;
    }
}

/// 对单个 Chunk 构造行迭代器。
pub fn NewIterator4Chunk(chk: Box<Chunk>) -> Box<Iterator4Chunk> {
    Box::new(Iterator4Chunk {
        chk,
        cursor: 0,
        numRows: 0,
    })
}

/// 单 Chunk 迭代器；`numRows` 在 `Begin` 时缓存，避免遍历中途行数变化带来歧义。
pub struct Iterator4Chunk {
    chk: Box<Chunk>,
    cursor: usize,
    numRows: usize,
}

impl Iterator for Iterator4Chunk {
    fn Begin(&mut self) -> Row {
        self.numRows = self.chk.NumRows();
        if self.numRows == 0 {
            return self.End();
        }
        self.cursor = 1;
        self.chk.GetRow(0)
    }

    fn Next(&mut self) -> Row {
        if self.cursor >= self.numRows {
            self.cursor = self.numRows + 1;
            return self.End();
        }
        let row = self.chk.GetRow(self.cursor);
        self.cursor += 1;
        row
    }

    fn Current(&mut self) -> Row {
        if self.cursor == 0 || self.cursor > self.Len() {
            self.End()
        } else {
            self.chk.GetRow(self.cursor - 1)
        }
    }

    fn End(&self) -> Row {
        Row::default()
    }
    fn ReachEnd(&mut self) {
        self.cursor = self.Len() + 1;
    }
    fn Len(&self) -> usize {
        self.chk.NumRows()
    }
    fn Error(&self) -> Option<ChunkError> {
        None
    }
}

impl Iterator4Chunk {
    /// 只读访问底层 Chunk。
    pub fn GetChunk(&self) -> &Chunk {
        &self.chk
    }
    /// 可变访问底层 Chunk。
    pub fn GetChunkMut(&mut self) -> &mut Chunk {
        &mut self.chk
    }
    /// 重置游标与缓存行数（不更换 Chunk）。
    pub fn Reset(&mut self) {
        self.cursor = 0;
        self.numRows = 0;
    }
    /// 替换底层 Chunk（不重置游标；调用方通常随后 `Reset`/`Begin`）。
    pub fn ResetChunk(&mut self, chk: Box<Chunk>) {
        self.chk = chk;
    }
}

/// 按 Chunk→行顺序遍历整个 `List`。
pub fn NewIterator4List(li: Box<List>) -> Box<dyn Iterator> {
    Box::new(iterator4List {
        li,
        chkCursor: 0,
        rowCursor: 0,
    })
}

/// List 迭代器：`(chkCursor, rowCursor)` 指向“下一行”。
struct iterator4List {
    li: Box<List>,
    chkCursor: usize,
    rowCursor: usize,
}

impl Iterator for iterator4List {
    fn Begin(&mut self) -> Row {
        if self.li.NumChunks() == 0 {
            return self.End();
        }
        let chunk = self.li.GetChunk(0);
        let row = chunk.GetRow(0);
        // 首块仅一行时下一位置落到下一块；否则留在同块第 1 行。
        if chunk.NumRows() == 1 {
            self.chkCursor = 1;
            self.rowCursor = 0;
        } else {
            self.chkCursor = 0;
            self.rowCursor = 1;
        }
        row
    }

    fn Next(&mut self) -> Row {
        if self.chkCursor >= self.li.NumChunks() {
            self.chkCursor = self.li.NumChunks() + 1;
            return self.End();
        }
        let chunk = self.li.GetChunk(self.chkCursor);
        let row = chunk.GetRow(self.rowCursor);
        self.rowCursor += 1;
        // 块内耗尽则跨到下一块第 0 行。
        if self.rowCursor == chunk.NumRows() {
            self.rowCursor = 0;
            self.chkCursor += 1;
        }
        row
    }

    fn Current(&mut self) -> Row {
        if (self.chkCursor == 0 && self.rowCursor == 0) || self.chkCursor > self.li.NumChunks() {
            return self.End();
        }
        // rowCursor==0 表示刚跨块，当前行是上一块末行。
        if self.rowCursor == 0 {
            let chunk = self.li.GetChunk(self.chkCursor - 1);
            return chunk.GetRow(chunk.NumRows() - 1);
        }
        self.li.GetChunk(self.chkCursor).GetRow(self.rowCursor - 1)
    }

    fn End(&self) -> Row {
        Row::default()
    }
    fn ReachEnd(&mut self) {
        self.chkCursor = self.li.NumChunks() + 1;
    }
    fn Len(&self) -> usize {
        self.li.Len()
    }
    fn Error(&self) -> Option<ChunkError> {
        None
    }
}

/// 按给定 `RowPtr` 序列从 `List` 取行（可乱序/子集）。
pub fn NewIterator4RowPtr(li: Box<List>, ptrs: Vec<RowPtr>) -> Box<dyn Iterator> {
    Box::new(iterator4RowPtr {
        li,
        ptrs,
        cursor: 0,
    })
}

/// 按指针表遍历；`Len` 为指针个数而非 List 总行数。
struct iterator4RowPtr {
    li: Box<List>,
    ptrs: Vec<RowPtr>,
    cursor: usize,
}

impl Iterator for iterator4RowPtr {
    fn Begin(&mut self) -> Row {
        if self.Len() == 0 {
            return self.End();
        }
        self.cursor = 1;
        self.li.GetRow(self.ptrs[0])
    }

    fn Next(&mut self) -> Row {
        let len = self.Len();
        if self.cursor >= len {
            self.cursor = len + 1;
            return self.End();
        }
        let row = self.li.GetRow(self.ptrs[self.cursor]);
        self.cursor += 1;
        row
    }

    fn Current(&mut self) -> Row {
        if self.cursor == 0 || self.cursor > self.Len() {
            self.End()
        } else {
            self.li.GetRow(self.ptrs[self.cursor - 1])
        }
    }

    fn End(&self) -> Row {
        Row::default()
    }
    fn ReachEnd(&mut self) {
        self.cursor = self.Len() + 1;
    }
    fn Len(&self) -> usize {
        self.ptrs.len()
    }
    fn Error(&self) -> Option<ChunkError> {
        None
    }
}

/// 遍历 `RowContainer`（可能触发磁盘读，错误经 `Error` 暴露）。
pub fn NewIterator4RowContainer(c: Box<RowContainer>) -> Box<dyn Iterator> {
    Box::new(iterator4RowContainer {
        c,
        chkIdx: 0,
        rowIdx: 0,
        err: None,
    })
}

/// RowContainer 迭代器；`rowIdx` 用 `isize` 以便 `Begin` 置 -1 再 `Next`。
struct iterator4RowContainer {
    c: Box<RowContainer>,
    chkIdx: usize,
    rowIdx: isize,
    err: Option<ChunkError>,
}

impl iterator4RowContainer {
    /// 推进到下一行；跨块时重置 `rowIdx`。
    fn setNextPtr(&mut self) {
        self.rowIdx += 1;
        if self.rowIdx as usize == self.c.NumRowsOfChunk(self.chkIdx) {
            self.rowIdx = 0;
            self.chkIdx += 1;
        }
    }
}

impl Iterator for iterator4RowContainer {
    fn Len(&self) -> usize {
        self.c.NumRow()
    }

    fn Begin(&mut self) -> Row {
        self.chkIdx = 0;
        self.rowIdx = -1;
        self.err = None;
        self.Next()
    }

    fn Next(&mut self) -> Row {
        if self.chkIdx >= self.c.NumChunks() {
            self.ReachEnd();
            return self.End();
        }
        self.setNextPtr();
        self.Current()
    }

    fn Current(&mut self) -> Row {
        if self.rowIdx < 0 || self.chkIdx >= self.c.NumChunks() {
            return self.End();
        }
        match self.c.GetRow(RowPtr {
            ChkIdx: self.chkIdx as u32,
            RowIdx: self.rowIdx as u32,
        }) {
            Ok(row) => row,
            Err(error) => {
                // 读失败时记录错误并停在 End，避免继续读。
                self.err = Some(error);
                self.ReachEnd();
                self.End()
            }
        }
    }

    fn End(&self) -> Row {
        Row::default()
    }
    fn ReachEnd(&mut self) {
        self.chkIdx = self.c.NumChunks();
        self.rowIdx = 0;
    }
    fn Error(&self) -> Option<ChunkError> {
        self.err.clone()
    }
}

/// 串联多个子迭代器；构造时丢弃空迭代器并累计 `length`。
struct multiIterator {
    iters: Vec<Box<dyn Iterator>>,
    numIter: usize,
    length: usize,
    curPtr: usize,
    err: Option<ChunkError>,
}

/// 过滤空输入后串联；`Len` 为各非空子迭代器行数之和。
pub fn NewMultiIterator(iters: Vec<Box<dyn Iterator>>) -> Box<dyn Iterator> {
    let mut kept = Vec::new();
    let mut length = 0;
    for iterator in iters {
        if iterator.Len() > 0 {
            length += iterator.Len();
            kept.push(iterator);
        }
    }
    let numIter = kept.len();
    Box::new(multiIterator {
        iters: kept,
        numIter,
        length,
        curPtr: 0,
        err: None,
    })
}

impl Iterator for multiIterator {
    fn Len(&self) -> usize {
        self.length
    }

    fn Begin(&mut self) -> Row {
        self.curPtr = 0;
        self.err = None;
        if self.numIter > 0 {
            self.iters[0].Begin();
        }
        self.Current()
    }

    fn Next(&mut self) -> Row {
        if self.curPtr == self.numIter {
            return self.End();
        }
        let mut next = self.iters[self.curPtr].Next();
        // 当前子迭代器耗尽则切换到下一个的 Begin；子错误则整体 ReachEnd。
        if next == self.iters[self.curPtr].End() {
            self.err = self.iters[self.curPtr].Error();
            if self.err.is_some() {
                self.ReachEnd();
                return self.End();
            }
            self.curPtr += 1;
            if self.curPtr == self.numIter {
                return self.End();
            }
            next = self.iters[self.curPtr].Begin();
        }
        next
    }

    fn Current(&mut self) -> Row {
        if self.curPtr == self.numIter {
            return self.End();
        }
        let row = self.iters[self.curPtr].Current();
        if row == self.iters[self.curPtr].End() {
            self.err = self.iters[self.curPtr].Error();
        }
        row
    }

    fn End(&self) -> Row {
        Row::default()
    }
    fn ReachEnd(&mut self) {
        self.curPtr = self.numIter;
    }
    fn Error(&self) -> Option<ChunkError> {
        self.err.clone()
    }
}
