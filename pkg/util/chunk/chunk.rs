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

// Chunk：执行引擎中按列（columnar）存放的一批行，布局接近 Apache Arrow。
//
// 提供创建/复用/追加/截断、selection vector（`sel`）过滤、列引用与交换，
// 以及按 Datum/各具体类型追加单元格。`requiredRows` 决定 `IsFull` 的满批阈值。

/// 当 Chunk 的 selection vector 非空（非 nil）时返回的错误信息。
/// 某些列交换/引用操作要求 `sel` 为空，否则会破坏物理行下标语义。
pub static MSG_ERR_SEL_NOT_NIL: &str =
    "The selection vector of Chunk is not nil. Please file a bug to the TiDB Team";

// Chunk 对应 Go 的同名结构：以 Apache Arrow 风格的列式布局存储多行数据。
// sel 为 Go 中可为 nil 的 selection vector；这里用 Option 保留 nil 和空切片的差异。
#[derive(Clone, Default)]
/// 列式多行容器：`columns` 存各列，`sel` 为可选的逻辑行下标映射。
pub struct Chunk {
    pub sel: Option<Vec<usize>>,
    pub columns: Vec<Column>,
    pub numVirtualRows: usize,
    pub capacity: usize,
    pub requiredRows: usize,
    pub inCompleteChunk: bool,
}

// Capacity constants.
/// 容量从 0 增长时的初始行容量。
pub const InitialCapacity: usize = 32;
/// 零容量常量。
pub const ZeroCapacity: usize = 0;

/// 将字段描述统一成 `types::FieldType`，兼容裸值与 `Box`。
pub trait IntoFieldType {
    fn into_field_type(self) -> types::FieldType;
}

impl IntoFieldType for types::FieldType {
    fn into_field_type(self) -> types::FieldType {
        self
    }
}

impl IntoFieldType for Box<types::FieldType> {
    fn into_field_type(self) -> types::FieldType {
        *self
    }
}

// NewEmptyChunk 对应 Go 的 NewEmptyChunk：按字段类型创建空列但不预分配行容量。
/// 按字段类型创建空列 Chunk，不预分配行容量。
pub fn NewEmptyChunk<T: IntoFieldType>(fields: Vec<T>) -> Box<Chunk> {
    let column_capacity = fields.len().max(1);
    let mut chk = Box::new(Chunk {
        sel: None,
        // A spare slot preserves Go's non-nil empty slice state when `fields`
        // is empty; `Vec::new()` represents a nil column slice internally.
        columns: Vec::with_capacity(column_capacity),
        numVirtualRows: 0,
        capacity: 0,
        requiredRows: 0,
        inCompleteChunk: false,
    });

    for f in fields {
        let f = f.into_field_type();
        chk.columns.push(*NewEmptyColumn(&f));
    }
    chk
}

// NewChunkWithCapacity 对应 Go 的同名函数：capacity 同时作为初始和最大行数传入 New。
/// 以相同初值与上限创建带容量的 Chunk。
pub fn NewChunkWithCapacity<T: IntoFieldType>(fields: Vec<T>, capacity: usize) -> Box<Chunk> {
    New(fields, capacity, capacity)
}

// NewChunkFromPoolWithCapacity 对应 Go 的对象池入口；这里保留池化调用形状。
/// 从对象池取出（或新建）指定初始容量的 Chunk。
pub fn NewChunkFromPoolWithCapacity<T: IntoFieldType>(fields: Vec<T>, initCap: usize) -> Box<Chunk> {
    getChunkFromPool(initCap, fields.into_iter().map(IntoFieldType::into_field_type).collect())
}

// New creates a new chunk.
// cap: the limit for the max number of rows.
// maxChunkSize: the max limit for the number of rows.
/// 创建新 Chunk；`capacity` 受 `maxChunkSize` 限制，`requiredRows` 默认为上限。
pub fn New<T: IntoFieldType>(fields: Vec<T>, capacity: usize, maxChunkSize: usize) -> Box<Chunk> {
    let real_capacity = std::cmp::min(capacity, maxChunkSize);
    let column_capacity = fields.len().max(1);
    let mut chk = Box::new(Chunk {
        sel: None,
        columns: Vec::with_capacity(column_capacity),
        numVirtualRows: 0,
        capacity: real_capacity,
        // Go 默认把 requiredRows 设成 maxChunkSize，使 IsFull 等价于 NumRows >= maxChunkSize。
        requiredRows: maxChunkSize,
        inCompleteChunk: false,
    });

    for f in fields {
        let f = f.into_field_type();
        chk.columns.push(*NewColumn(&f, chk.capacity));
    }
    chk
}

// renewWithCapacity creates a new Chunk based on an existing Chunk with capacity.
// 新 Chunk 保留旧 Chunk 的 schema 和 inCompleteChunk 标记，但不复制行数据。
/// 基于已有 Chunk 的 schema 重建空数据 Chunk，并指定容量与 requiredRows。
pub fn renewWithCapacity(chk: &Chunk, capacity: usize, requiredRows: usize) -> Box<Chunk> {
    if chk.columns.is_empty() && chk.columns.capacity() == 0 {
        return Box::new(Chunk {
            sel: None,
            columns: Vec::new(),
            numVirtualRows: 0,
            capacity: 0,
            requiredRows: 0,
            inCompleteChunk: chk.inCompleteChunk,
        });
    }
    Box::new(Chunk {
        sel: None,
        columns: renewColumns(&chk.columns, capacity),
        numVirtualRows: 0,
        capacity,
        requiredRows,
        inCompleteChunk: chk.inCompleteChunk,
    })
}

// Renew creates a new Chunk based on an existing Chunk.
/// 按当前行数重算容量后重建空 Chunk。
pub fn Renew(chk: &Chunk, maxChunkSize: usize) -> Box<Chunk> {
    let newCap = reCalcCapacity(chk, maxChunkSize);
    renewWithCapacity(chk, newCap, maxChunkSize)
}

// renewColumns creates the columns of a Chunk.
/// 按旧列类型尺寸重建空列向量。
pub fn renewColumns(oldCol: &[Column], capacity: usize) -> Vec<Column> {
    let mut columns = Vec::with_capacity(oldCol.len().max(1));
    for col in oldCol {
        columns.push(*newColumn(col.typeSize(), capacity));
    }
    columns
}

// renewEmpty creates a new Chunk based on an existing Chunk but keep columns empty.
/// 复制元信息与 `sel`，但列数据为空。
pub fn renewEmpty(chk: &Chunk) -> Box<Chunk> {
    let mut newChk = Box::new(Chunk {
        sel: None,
        columns: Vec::new(),
        numVirtualRows: chk.numVirtualRows,
        capacity: chk.capacity,
        requiredRows: chk.requiredRows,
        inCompleteChunk: chk.inCompleteChunk,
    });
    if let Some(sel) = &chk.sel {
        newChk.sel = Some(sel.clone());
    }
    newChk
}

impl Chunk {
    // resetForReuse 对应 Go 中归还池前的清理：只留下 columns 的空容量，其他字段归零。
    /// 归还对象池前清理：列槽位清空，其余字段归零。
    pub fn resetForReuse(&mut self) {
        for col in &mut self.columns {
            // Go 这里把 []*Column 中的元素置 nil，避免旧列继续被引用。
            *col = Column::default();
        }
        self.columns.clear();
        self.sel = None;
        self.numVirtualRows = 0;
        self.capacity = 0;
        self.requiredRows = 0;
        self.inCompleteChunk = false;
    }

    // SetInCompleteChunk will set c.inCompleteChunk, used in join.
    /// 标记是否为 join 场景下的不完整 Chunk。
    pub fn SetInCompleteChunk(&mut self, isInCompleteChunk: bool) {
        self.inCompleteChunk = isInCompleteChunk;
    }

    // IsInCompleteChunk returns true if this chunk is inCompleteChunk, used only in test.
    /// 返回 `inCompleteChunk` 标记（主要用于测试）。
    pub fn IsInCompleteChunk(&self) -> bool {
        self.inCompleteChunk
    }

    // GetNumVirtualRows return c.numVirtualRows, used only in test.
    /// 返回虚拟行数（主要用于测试）。
    pub fn GetNumVirtualRows(&self) -> usize {
        self.numVirtualRows
    }

    // MemoryUsage returns the total memory usage of a Chunk in bytes.
    // Go 使用 unsafe.Sizeof 加各切片容量；保留同样估算口径。
    /// 估算 Chunk 占用的内存字节数（对齐 Go 的 capacity 口径）。
    pub fn MemoryUsage(&self) -> i64 {
        let mut sum: i64 = 0;
        for col in &self.columns {
            sum += std::mem::size_of_val(col) as i64;
            sum += col.nullBitmap.capacity() as i64;
            sum += (col.offsets.capacity() * 8) as i64;
            sum += col.data.capacity() as i64;
            sum += col.elemBuf.capacity() as i64;
        }
        sum
    }

    // RequiredRows returns how many rows is considered full.
    /// 视为“满批”所需的行数。
    pub fn RequiredRows(&self) -> usize {
        self.requiredRows
    }

    // SetRequiredRows sets the number of required rows.
    /// 设置满批行数，越界或 0 时回落到 `maxChunkSize`。
    pub fn SetRequiredRows(&mut self, mut requiredRows: usize, maxChunkSize: usize) -> &mut Chunk {
        if requiredRows == 0 || requiredRows > maxChunkSize {
            requiredRows = maxChunkSize;
        }
        self.requiredRows = requiredRows;
        self
    }

    // IsFull returns if this chunk is considered full.
    /// 当前行数是否达到 `requiredRows`。
    pub fn IsFull(&self) -> bool {
        self.NumRows() >= self.requiredRows
    }

    // Prune creates a new Chunk according to `c` and prunes unused columns.
    /// 按用到的列下标投影出新 Chunk（列引用复用）。
    pub fn Prune(&self, usedColIdxs: &[usize]) -> Box<Chunk> {
        let mut chk = renewEmpty(self);
        chk.columns = Vec::with_capacity(usedColIdxs.len().max(1));
        for idx in usedColIdxs {
            // Go 直接复用列指针；用 clone 表示共享引用语义需要后续接线。
            chk.columns.push(self.columns[*idx].reference_clone());
        }
        chk
    }

    // MakeRef makes Column in "dstColIdx" reference to Column in "srcColIdx".
    /// 让目标列引用源列的底层存储。
    pub fn MakeRef(&mut self, srcColIdx: usize, dstColIdx: usize) {
        self.columns[dstColIdx] = self.columns[srcColIdx].reference_clone();
    }

    // MakeRefTo copies columns `src.columns[srcColIdx]` to `c.columns[dstColIdx]`.
    /// 把 `src` 的某列引用到本 Chunk 的目标列；`sel` 非空时报错。
    pub fn MakeRefTo(
        &mut self,
        dstColIdx: usize,
        src: &Chunk,
        srcColIdx: usize,
    ) -> Result<(), errors::Error> {
        if self.sel.is_some() || src.sel.is_some() {
            return Err(errors::New(MSG_ERR_SEL_NOT_NIL));
        }
        self.columns[dstColIdx] = src.columns[srcColIdx].reference_clone();
        Ok(())
    }

    // swapColumn swaps Column "c.columns[colIdx]" with Column "other.columns[otherIdx]".
    // Go 需要找到共享列引用的最左位置并重建引用；这里按同样顺序保留该保护逻辑。
    /// 与另一 Chunk 交换列，并修复同源列引用链。
    pub fn swapColumn(
        &mut self,
        mut colIdx: usize,
        other: &mut Chunk,
        mut otherIdx: usize,
    ) -> Result<(), errors::Error> {
        if self.sel.is_some() || other.sel.is_some() {
            return Err(errors::New(MSG_ERR_SEL_NOT_NIL));
        }

        for i in 0..colIdx {
            if self.columns[i].same_ref(&self.columns[colIdx]) {
                colIdx = i;
                break;
            }
        }
        for i in 0..otherIdx {
            if other.columns[i].same_ref(&other.columns[otherIdx]) {
                otherIdx = i;
                break;
            }
        }

        let mut refColsIdx = Vec::with_capacity(self.columns.len().saturating_sub(colIdx));
        for i in colIdx..self.columns.len() {
            if self.columns[i].same_ref(&self.columns[colIdx]) {
                refColsIdx.push(i);
            }
        }
        let mut refColsIdx4Other = Vec::with_capacity(other.columns.len().saturating_sub(otherIdx));
        for i in otherIdx..other.columns.len() {
            if other.columns[i].same_ref(&other.columns[otherIdx]) {
                refColsIdx4Other.push(i);
            }
        }

        std::mem::swap(&mut self.columns[colIdx], &mut other.columns[otherIdx]);

        // 交换后把所有曾经引用同一底层列的输出列重新指回新的主列。
        for i in refColsIdx {
            self.MakeRef(colIdx, i);
        }
        for i in refColsIdx4Other {
            other.MakeRef(otherIdx, i);
        }
        Ok(())
    }

    // swapColumnWithin preserves Go's same-Chunk call shape without creating
    // two aliased Rust mutable references to the same Chunk.
    /// 在同一 Chunk 内交换两列，避免双重可变借用别名问题。
    pub fn swapColumnWithin(
        &mut self,
        mut colIdx: usize,
        mut otherIdx: usize,
    ) -> Result<(), errors::Error> {
        if self.sel.is_some() {
            return Err(errors::New(MSG_ERR_SEL_NOT_NIL));
        }
        for i in 0..colIdx {
            if self.columns[i].same_ref(&self.columns[colIdx]) {
                colIdx = i;
                break;
            }
        }
        for i in 0..otherIdx {
            if self.columns[i].same_ref(&self.columns[otherIdx]) {
                otherIdx = i;
                break;
            }
        }
        let refColsIdx = (colIdx..self.columns.len())
            .filter(|&i| self.columns[i].same_ref(&self.columns[colIdx]))
            .collect::<Vec<_>>();
        let refColsIdx4Other = (otherIdx..self.columns.len())
            .filter(|&i| self.columns[i].same_ref(&self.columns[otherIdx]))
            .collect::<Vec<_>>();
        self.columns.swap(colIdx, otherIdx);
        for i in refColsIdx {
            self.MakeRef(colIdx, i);
        }
        for i in refColsIdx4Other {
            self.MakeRef(otherIdx, i);
        }
        Ok(())
    }

    // SwapColumns swaps columns with another Chunk.
    /// 与另一 Chunk 整体交换 `sel`/列/虚拟行数。
    pub fn SwapColumns(&mut self, other: &mut Chunk) {
        std::mem::swap(&mut self.sel, &mut other.sel);
        std::mem::swap(&mut self.columns, &mut other.columns);
        std::mem::swap(&mut self.numVirtualRows, &mut other.numVirtualRows);
    }

    // SetNumVirtualRows sets the virtual row number for a Chunk.
    /// 设置虚拟行数（无物理列时表示逻辑行数）。
    pub fn SetNumVirtualRows(&mut self, numVirtualRows: usize) {
        self.numVirtualRows = numVirtualRows;
    }

    // Reset resets the chunk, so the memory it allocated can be reused.
    /// 清空行数据以复用已分配内存。
    pub fn Reset(&mut self) {
        self.sel = None;
        for col in &mut self.columns {
            col.reset();
        }
        self.numVirtualRows = 0;
    }

    // CopyConstruct creates a new chunk and copies this chunk's data into it.
    /// 深拷贝当前 Chunk 的全部列数据。
    pub fn CopyConstruct(&self) -> Box<Chunk> {
        let mut newChk = renewEmpty(self);
        newChk.columns = Vec::with_capacity(self.columns.len().max(1));
        for col in &self.columns {
            newChk.columns.push(*col.CopyConstruct(None));
        }
        newChk
    }

    // CopyConstructSel is just like CopyConstruct, but ignores unselected rows.
    /// 深拷贝，但只保留 `sel` 选中的行。
    pub fn CopyConstructSel(&self) -> Box<Chunk> {
        if self.sel.is_none() {
            return self.CopyConstruct();
        }
        let mut newChk = renewWithCapacity(self, self.capacity, self.requiredRows);
        for colIdx in 0..newChk.columns.len() {
            for rowIdx in self.sel.as_ref().unwrap() {
                appendCellByCell(&mut newChk.columns[colIdx], &self.columns[colIdx], *rowIdx);
            }
        }
        newChk
    }

    // GrowAndReset resets the Chunk and doubles the capacity of the Chunk.
    /// 清空数据并在需要时倍增容量。
    pub fn GrowAndReset(&mut self, maxChunkSize: usize) {
        self.sel = None;
        if self.columns.is_empty() && self.columns.capacity() == 0 {
            return;
        }
        let newCap = reCalcCapacity(self, maxChunkSize);
        if newCap <= self.capacity {
            self.Reset();
            return;
        }
        self.capacity = newCap;
        self.columns = renewColumns(&self.columns, newCap);
        self.numVirtualRows = 0;
        self.requiredRows = maxChunkSize;
    }

    // Capacity returns the capacity of the Chunk.
    /// 返回行容量上限。
    pub fn Capacity(&self) -> usize {
        self.capacity
    }

    // NumCols returns the number of columns in the chunk.
    /// 返回列数。
    pub fn NumCols(&self) -> usize {
        self.columns.len()
    }

    // NumRows returns the number of rows in the chunk.
    /// 返回逻辑行数：优先 `sel` 长度，否则看首列或虚拟行。
    pub fn NumRows(&self) -> usize {
        if let Some(sel) = &self.sel {
            return sel.len();
        }
        if self.inCompleteChunk || self.NumCols() == 0 {
            return self.numVirtualRows;
        }
        self.columns[0].length
    }

    // GetRow gets the Row in the chunk with the row index.
    /// 按逻辑行下标取 `Row` 视图。
    pub fn GetRow(&self, idx: usize) -> Row {
        if let Some(sel) = &self.sel {
            // Go 把逻辑行号映射成物理行号，迭代时自动跳过被过滤的行。
            return Row::view(self, sel[idx]);
        }
        Row::view(self, idx)
    }

    // AppendRow appends a row to the chunk.
    /// 追加一整行并增加虚拟行计数。
    pub fn AppendRow(&mut self, row: Row) {
        self.AppendPartialRow(0, row);
        self.numVirtualRows += 1;
    }

    // AppendPartialRow appends a row to the chunk, starting from colOff.
    /// 从 `colOff` 起追加部分列单元格。
    pub fn AppendPartialRow(&mut self, colOff: usize, row: Row) {
        self.appendSel(colOff);
        for (i, rowCol) in unsafe { &*row.c }.columns.iter().enumerate() {
            appendCellByCell(&mut self.columns[colOff + i], rowCol, row.idx);
        }
    }

    // AppendRowsByColIdxs appends multiple rows by its colIdxs to the chunk.
    /// 按列下标集合追加多行，返回写入的单元格数。
    pub fn AppendRowsByColIdxs(&mut self, rows: &[Row], colIdxs: Option<&[usize]>) -> usize {
        if colIdxs.is_none() {
            if rows.is_empty() {
                return 0;
            }
            self.AppendRows(rows);
            return rows[0].Len() * rows.len();
        }

        let colIdxs = colIdxs.unwrap();
        for srcRow in rows {
            self.appendSel(0);
            for (i, colIdx) in colIdxs.iter().enumerate() {
                appendCellByCell(
                    &mut self.columns[i],
                    &unsafe { &*srcRow.c }.columns[*colIdx],
                    srcRow.idx,
                );
            }
        }
        self.numVirtualRows += rows.len();
        colIdxs.len() * rows.len()
    }

    // AppendRowByColIdxs appends a row to the chunk using selected columns.
    /// 按列下标集合追加一行。
    pub fn AppendRowByColIdxs(&mut self, row: Row, colIdxs: Option<&[usize]>) -> usize {
        let wide = self.AppendPartialRowByColIdxs(0, row, colIdxs);
        self.numVirtualRows += 1;
        wide
    }

    // AppendPartialRowByColIdxs appends a row to the chunk starting from colOff.
    /// 从 `colOff` 起按列下标追加一行的部分列。
    pub fn AppendPartialRowByColIdxs(
        &mut self,
        colOff: usize,
        row: Row,
        colIdxs: Option<&[usize]>,
    ) -> usize {
        if colIdxs.is_none() {
            let width = row.Len();
            self.AppendPartialRow(colOff, row);
            return width;
        }

        self.appendSel(colOff);
        let colIdxs = colIdxs.unwrap();
        for (i, colIdx) in colIdxs.iter().enumerate() {
            appendCellByCell(
                &mut self.columns[colOff + i],
                &unsafe { &*row.c }.columns[*colIdx],
                row.idx,
            );
        }
        colIdxs.len()
    }

    // Append appends rows in [begin, end) in another Chunk to a Chunk.
    /// 追加另一 Chunk 中 `[begin, end)` 的物理行区间。
    pub fn Append(&mut self, other: &Chunk, begin: usize, end: usize) {
        for colID in 0..other.columns.len() {
            let src = &other.columns[colID];
            for _ in begin..end {
                self.appendSel(colID);
            }
            let dst = &mut self.columns[colID];
            if src.IsFixed() {
                let elemLen = src.elemBuf.len();
                dst.data
                    .extend_from_slice(&src.data[begin * elemLen..end * elemLen]);
            } else {
                let beginOffset = src.offsets[begin] as usize;
                let endOffset = src.offsets[end] as usize;
                dst.data
                    .extend_from_slice(&src.data[beginOffset..endOffset]);
                let mut lastOffset = *dst.offsets.last().unwrap_or(&0);
                for i in begin..end {
                    lastOffset += src.offsets[i + 1] - src.offsets[i];
                    dst.offsets.push(lastOffset);
                }
            }
            for i in begin..end {
                dst.appendNullBitmap(!src.IsNull(i));
                dst.length += 1;
            }
        }
        self.numVirtualRows += end - begin;
    }

    // TruncateTo truncates rows from tail to head in a Chunk to "numRows" rows.
    /// 截断到指定行数，并整理 nullBitmap 尾部位。
    pub fn TruncateTo(&mut self, numRows: usize) {
        self.Reconstruct();
        for col in &mut self.columns {
            if col.IsFixed() {
                let elemLen = col.elemBuf.len();
                col.data.truncate(numRows * elemLen);
            } else {
                col.data.truncate(col.offsets[numRows] as usize);
                col.offsets.truncate(numRows + 1);
            }
            col.length = numRows;
            let bitmapLen = (col.length + 7) / 8;
            col.nullBitmap.truncate(bitmapLen);
            if col.length % 8 != 0 {
                // Go 追加 null 时只增长 nullCount，截断时要清掉最后一个字节的无效高位。
                let unusedBitsLen = 8 - (col.length % 8);
                let idx = bitmapLen - 1;
                col.nullBitmap[idx] <<= unusedBitsLen;
                col.nullBitmap[idx] >>= unusedBitsLen;
            }
        }
        self.numVirtualRows = numRows;
    }

    // AppendNull appends a null value to the chunk.
    /// 向指定列追加 NULL。
    pub fn AppendNull(&mut self, colIdx: usize) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendNull();
    }

    // AppendInt64 appends a int64 value to the chunk.
    /// 向指定列追加 `i64`。
    pub fn AppendInt64(&mut self, colIdx: usize, i: i64) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendInt64(i);
    }

    // AppendUint64 appends a uint64 value to the chunk.
    /// 向指定列追加 `u64`。
    pub fn AppendUint64(&mut self, colIdx: usize, u: u64) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendUint64(u);
    }

    // AppendFloat32 appends a float32 value to the chunk.
    /// 向指定列追加 `f32`。
    pub fn AppendFloat32(&mut self, colIdx: usize, f: f32) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendFloat32(f);
    }

    // AppendFloat64 appends a float64 value to the chunk.
    /// 向指定列追加 `f64`。
    pub fn AppendFloat64(&mut self, colIdx: usize, f: f64) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendFloat64(f);
    }

    // AppendString appends a string value to the chunk.
    /// 向指定列追加字符串。
    pub fn AppendString(&mut self, colIdx: usize, str_: &str) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendString(str_);
    }

    // AppendBytes appends a bytes value to the chunk.
    /// 向指定列追加字节串。
    pub fn AppendBytes(&mut self, colIdx: usize, b: &[u8]) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendBytes(b);
    }

    /// AppendRaw appends one already-encoded cell. It is used by the row-on-disk
    /// decoder, which stores fixed-width values without reinterpreting them.
    pub fn AppendRaw(&mut self, colIdx: usize, raw: &[u8]) {
        self.appendSel(colIdx);
        let column = &mut self.columns[colIdx];
        if column.IsFixed() {
            assert_eq!(raw.len(), column.elemBuf.len());
            column.AppendNullBitmap(true);
            column.data.extend_from_slice(raw);
            column.length += 1;
        } else {
            column.AppendBytes(raw);
        }
    }

    // AppendTime appends a Time value to the chunk.
    /// 向指定列追加时间类型值。
    pub fn AppendTime(&mut self, colIdx: usize, t: types::Time) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendTime(t);
    }

    // AppendDuration appends a Duration value to the chunk. Fsp is ignored.
    /// 向指定列追加 Duration（忽略 Fsp）。
    pub fn AppendDuration(&mut self, colIdx: usize, dur: types::Duration) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendDuration(dur);
    }

    // AppendMyDecimal appends a MyDecimal value to the chunk.
    /// 向指定列追加 MyDecimal。
    pub fn AppendMyDecimal(&mut self, colIdx: usize, dec: &types::MyDecimal) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendMyDecimal(dec);
    }

    // AppendEnum appends an Enum value to the chunk.
    /// 向指定列追加 Enum。
    pub fn AppendEnum(&mut self, colIdx: usize, enum_: types::Enum) {
        self.appendSel(colIdx);
        self.columns[colIdx].appendNameValue(&enum_.Name, enum_.Value);
    }

    // AppendSet appends a Set value to the chunk.
    /// 向指定列追加 Set。
    pub fn AppendSet(&mut self, colIdx: usize, set: types::Set) {
        self.appendSel(colIdx);
        self.columns[colIdx].appendNameValue(&set.Name, set.Value);
    }

    // AppendJSON appends a JSON value to the chunk.
    /// 向指定列追加 JSON。
    pub fn AppendJSON(&mut self, colIdx: usize, j: types::BinaryJSON) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendJSON(j);
    }

    // AppendVectorFloat32 appends a VectorFloat32 value to the chunk.
    /// 向指定列追加向量浮点。
    pub fn AppendVectorFloat32(&mut self, colIdx: usize, v: types::VectorFloat32) {
        self.appendSel(colIdx);
        self.columns[colIdx].AppendVectorFloat32(v);
    }

    /// 若存在 `sel` 且正在写第 0 列，则把新物理行下标记入 selection。
    pub fn appendSel(&mut self, colIdx: usize) {
        if colIdx == 0 && self.sel.is_some() {
            let row = self.columns[0].length;
            self.sel.as_mut().unwrap().push(row);
        }
    }

    // AppendDatum appends a datum into the chunk.
    /// 按 Datum 动态类型分发追加到指定列。
    pub fn AppendDatum(&mut self, colIdx: usize, d: &types::Datum) {
        match d.Kind() {
            types::KindNull => self.AppendNull(colIdx),
            types::KindInt64 => self.AppendInt64(colIdx, d.GetInt64()),
            types::KindUint64 => self.AppendUint64(colIdx, d.GetUint64()),
            types::KindFloat32 => self.AppendFloat32(colIdx, d.GetFloat32()),
            types::KindFloat64 => self.AppendFloat64(colIdx, d.GetFloat64()),
            types::KindString
            | types::KindBytes
            | types::KindBinaryLiteral
            | types::KindRaw
            | types::KindMysqlBit => self.AppendBytes(colIdx, &d.GetBytes()),
            types::KindMysqlDecimal => self.AppendMyDecimal(colIdx, &d.GetMysqlDecimal()),
            types::KindMysqlDuration => self.AppendDuration(colIdx, d.GetMysqlDuration()),
            types::KindMysqlEnum => self.AppendEnum(colIdx, d.GetMysqlEnum()),
            types::KindMysqlSet => self.AppendSet(colIdx, d.GetMysqlSet()),
            types::KindMysqlTime => self.AppendTime(colIdx, d.GetMysqlTime()),
            types::KindMysqlJSON => self.AppendJSON(colIdx, d.GetMysqlJSON()),
            types::KindVectorFloat32 => self.AppendVectorFloat32(colIdx, d.GetVectorFloat32()),
            _ => {}
        }
    }

    // Column returns the specific column.
    /// 返回指定下标的列引用。
    pub fn Column(&self, colIdx: usize) -> &Column {
        &self.columns[colIdx]
    }

    // SetCol sets the colIdx Column to col and returns the old Column.
    /// 替换列；若与原列同引用则返回 `None`。
    pub fn SetCol(&mut self, colIdx: usize, col: Column) -> Option<Column> {
        if col.same_ref(&self.columns[colIdx]) {
            return None;
        }
        Some(std::mem::replace(&mut self.columns[colIdx], col))
    }

    // Sel returns Sel of this Chunk.
    /// 返回 selection vector。
    pub fn Sel(&self) -> Option<&[usize]> {
        self.sel.as_deref()
    }

    // SetSel sets a Sel for this Chunk.
    /// 设置 selection vector。
    pub fn SetSel(&mut self, sel: Option<Vec<usize>>) {
        self.sel = sel;
    }

    // CloneEmpty returns an empty chunk that has the same schema with current chunk.
    /// 克隆同 schema 的空 Chunk。
    pub fn CloneEmpty(&self, maxCapacity: usize) -> Box<Chunk> {
        renewWithCapacity(self, maxCapacity, maxCapacity)
    }

    // Reconstruct removes all filtered rows in this Chunk.
    /// 物化 `sel`：去掉被过滤行并清空 selection。
    pub fn Reconstruct(&mut self) {
        if self.sel.is_none() {
            return;
        }
        let sel = self.sel.clone().unwrap();
        for col in &mut self.columns {
            col.reconstruct(&sel);
        }
        self.numVirtualRows = sel.len();
        self.sel = None;
    }

    // ToString returns all the values in a chunk.
    /// 将全部行格式化为调试字符串。
    pub fn ToString(&self, ft: Vec<types::FieldType>) -> String {
        let mut buf = String::with_capacity(self.NumRows() * 2);
        for rowIdx in 0..self.NumRows() {
            let row = self.GetRow(rowIdx);
            buf.push_str(&row.ToString(&ft));
            buf.push('\n');
        }
        buf
    }

    // AppendRows appends multiple rows to the chunk.
    /// 追加多行完整数据。
    pub fn AppendRows(&mut self, rows: &[Row]) {
        self.AppendPartialRows(0, rows);
        self.numVirtualRows += rows.len();
    }

    // AppendPartialRows appends multiple rows to the chunk.
    /// 从 `colOff` 起追加多行的部分列。
    pub fn AppendPartialRows(&mut self, colOff: usize, rows: &[Row]) {
        for i in 0..self.columns[colOff..].len() {
            for srcRow in rows {
                if i == 0 {
                    self.appendSel(colOff);
                }
                appendCellByCell(
                    &mut self.columns[colOff + i],
                    &unsafe { &*srcRow.c }.columns[i],
                    srcRow.idx,
                );
            }
        }
    }

    // Destroy is to destroy the Chunk and put Chunk into the pool.
    /// 销毁并归还到对象池。
    pub fn Destroy(self, initCap: usize, fields: Vec<types::FieldType>) {
        putChunkFromPool(initCap, fields, self)
    }
}

// reCalcCapacity calculates the capacity for another Chunk based on the current Chunk.
/// 根据当前是否已满决定是否倍增容量，且不超过 `maxChunkSize`。
pub fn reCalcCapacity(c: &Chunk, maxChunkSize: usize) -> usize {
    if c.NumRows() < c.capacity {
        return c.capacity;
    }
    let mut newCapacity = c.capacity * 2;
    if newCapacity == 0 {
        newCapacity = InitialCapacity;
    }
    std::cmp::min(newCapacity, maxChunkSize)
}

// appendCellByCell appends the cell with rowIdx of src into dst.
/// 把源列某一物理行的单元格追加到目标列。
pub fn appendCellByCell(dst: &mut Column, src: &Column, rowIdx: usize) {
    dst.appendNullBitmap(!src.IsNull(rowIdx));
    if src.IsFixed() {
        let elemLen = src.elemBuf.len();
        let offset = rowIdx * elemLen;
        dst.data
            .extend_from_slice(&src.data[offset..offset + elemLen]);
    } else {
        let start = src.offsets[rowIdx] as usize;
        let end = src.offsets[rowIdx + 1] as usize;
        dst.data.extend_from_slice(&src.data[start..end]);
        dst.offsets.push(dst.data.len() as i64);
    }
    dst.length += 1;
}

// AppendCellFromRawData appends the cell from raw data.
// Go 使用 unsafe.Pointer 直接从行格式内存取字节；只保留指针偏移和固定/变长分支。
/// 从行格式原始内存追加一个单元格，返回新的字节偏移。
pub fn AppendCellFromRawData(
    dst: &mut Column,
    rowData: *const u8,
    mut currentOffset: usize,
) -> usize {
    if dst.IsFixed() {
        let elemLen = dst.elemBuf.len();
        let bytes = unsafe { std::slice::from_raw_parts(rowData.add(currentOffset), elemLen) };
        dst.data.extend_from_slice(bytes);
        currentOffset += elemLen;
    } else {
        let elemLen = unsafe { *(rowData.add(currentOffset) as *const u32) } as usize;
        if elemLen > 0 {
            let bytes = unsafe {
                std::slice::from_raw_parts(rowData.add(currentOffset + sizeUint32), elemLen)
            };
            dst.data.extend_from_slice(bytes);
        }
        dst.offsets.push(dst.data.len() as i64);
        currentOffset += elemLen + sizeUint32;
    }
    dst.length += 1;
    currentOffset
}
