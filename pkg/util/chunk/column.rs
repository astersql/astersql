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

// 单列 Arrow 风格存储：null bitmap、定长/变长 data、offsets 与各类追加/读取 API。
//
// 对应 Go `pkg/util/chunk/column.go`。`Column` 是 Chunk 的列存储单元：定长类型用
// `elemBuf` 暂存元素再写入 `data`；变长类型用 `offsets` 标记区间。nullBitmap 中
// bit 0 表示 null、bit 1 表示非 null。另提供 Resize/Reconstruct/MergeNulls 等批量操作。

// Column 的 Arrow 风格内存布局、null bitmap、offsets、固定/变长值追加和读取。

// For varLenColumn (e.g. varchar), the accurate length of an element is unknown.
/// 变长列预估单元素字节数（真实长度未知时用于容量估算）。
pub const estimatedElemLen: usize = 8;

// Column stores one column of data in Apache Arrow format.
// nullBitmap 中 bit 0 表示 null，bit 1 表示 not null；offsets 只用于变长列。
/// 一列数据的 Arrow 布局容器；`reference_id` 用于同列指针相等性判断（对齐 Go `*Column`）。
pub struct Column {
    pub length: usize,
    pub nullBitmap: Vec<u8>,
    pub offsets: Vec<i64>,
    pub data: Vec<u8>,
    pub elemBuf: Vec<u8>,
    pub avoidReusing: bool,
    pub(crate) reference_id: usize,
}

static NEXT_COLUMN_REFERENCE_ID: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(1);

/// 分配全局递增的列引用 ID（Clone 时重新分配，避免与源列 `same_ref`）。
pub(crate) fn new_column_reference_id() -> usize {
    NEXT_COLUMN_REFERENCE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Default for Column {
    fn default() -> Self {
        Self {
            length: 0,
            nullBitmap: Vec::new(),
            offsets: Vec::new(),
            data: Vec::new(),
            elemBuf: Vec::new(),
            avoidReusing: false,
            reference_id: new_column_reference_id(),
        }
    }
}

impl Clone for Column {
    fn clone(&self) -> Self {
        Self {
            length: self.length,
            nullBitmap: self.nullBitmap.clone(),
            offsets: self.offsets.clone(),
            data: self.data.clone(),
            elemBuf: self.elemBuf.clone(),
            avoidReusing: self.avoidReusing,
            reference_id: new_column_reference_id(),
        }
    }
}

// ColumnAllocator defines an allocator for Column.
/// 按字段类型与容量分配 `Column` 的抽象。
pub trait ColumnAllocator {
    fn NewColumn(&self, ft: &types::FieldType, count: usize) -> Box<Column>;
}

// DefaultColumnAllocator is the default implementation of ColumnAllocator.
/// 默认列分配器：根据 `getFixedLen` 选择定长或变长布局。
pub struct DefaultColumnAllocator;

// NewColumn implements the ColumnAllocator interface.
impl ColumnAllocator for DefaultColumnAllocator {
    fn NewColumn(&self, ft: &types::FieldType, capacity: usize) -> Box<Column> {
        newColumn(getFixedLen(ft), capacity)
    }
}

// NewEmptyColumn creates a new column with nothing.
/// 按字段类型创建空列：定长预分配 `elemBuf`，变长写入初始 offset 0。
pub fn NewEmptyColumn(ft: &types::FieldType) -> Box<Column> {
    let elemLen = getFixedLen(ft);
    let mut col = Column::default();
    if elemLen != VarElemLen {
        col.elemBuf = vec![0; elemLen];
    } else {
        col.offsets.push(0);
    }
    Box::new(col)
}

// NewColumn creates a new column with the specific type and capacity.
/// 按字段类型与容量创建列。
pub fn NewColumn(ft: &types::FieldType, capacity: usize) -> Box<Column> {
    newColumn(getFixedLen(ft), capacity)
}

/// `ts == VarElemLen` 时建变长列，否则建定长列。
pub fn newColumn(ts: usize, capacity: usize) -> Box<Column> {
    if ts == VarElemLen {
        newVarLenColumn(capacity)
    } else {
        newFixedLenColumn(ts, capacity)
    }
}

// newFixedLenColumn creates a fixed length Column with elemLen and initial data capacity.
pub fn newFixedLenColumn(elemLen: usize, capacity: usize) -> Box<Column> {
    Box::new(Column {
        length: 0,
        nullBitmap: Vec::with_capacity(getInitNullBitmapCap(capacity) as usize),
        offsets: Vec::new(),
        data: Vec::with_capacity(getInitDataMemCap(capacity, elemLen) as usize),
        elemBuf: vec![0; elemLen],
        avoidReusing: false,
        reference_id: new_column_reference_id(),
    })
}

// newVarLenColumn creates a variable length Column with initial data capacity.
pub fn newVarLenColumn(capacity: usize) -> Box<Column> {
    let mut offsets = Vec::with_capacity(getInitOffsetsCap(capacity) as usize);
    offsets.push(0);
    Box::new(Column {
        length: 0,
        nullBitmap: Vec::with_capacity(getInitNullBitmapCap(capacity) as usize),
        offsets,
        data: Vec::with_capacity(getInitDataMemCap(capacity, estimatedElemLen) as usize),
        elemBuf: Vec::new(),
        avoidReusing: false,
        reference_id: new_column_reference_id(),
    })
}

pub fn getInitDataMemCap(capacity: usize, elemLen: usize) -> i64 {
    (elemLen * capacity) as i64
}

pub fn getInitNullBitmapCap(capacity: usize) -> i64 {
    ((capacity + 7) >> 3) as i64
}

pub fn getInitOffsetsCap(capacity: usize) -> i64 {
    (capacity + 1) as i64
}

impl Column {
    // AppendDuration appends a duration value into this Column. Fsp is ignored.
    pub fn AppendDuration(&mut self, dur: types::Duration) {
        self.AppendInt64(dur.Duration as i64);
    }

    // AppendMyDecimal appends a MyDecimal value into this Column.
    pub fn AppendMyDecimal(&mut self, dec: &types::MyDecimal) {
        self.elemBuf = decimal_to_bytes(dec);
        self.finishAppendFixed();
    }

    pub fn appendNameValue(&mut self, name: &str, val: u64) {
        self.data.extend_from_slice(&val.to_ne_bytes());
        self.data.extend_from_slice(name.as_bytes());
        self.finishAppendVar();
    }

    // AppendJSON appends a BinaryJSON value into this Column.
    pub fn AppendJSON(&mut self, j: types::BinaryJSON) {
        self.data.push(j.TypeCode);
        self.data.extend_from_slice(&j.Value);
        self.finishAppendVar();
    }

    // AppendVectorFloat32 appends a VectorFloat32 value into this Column.
    pub fn AppendVectorFloat32(&mut self, v: types::VectorFloat32) {
        self.data = v.SerializeTo(std::mem::take(&mut self.data));
        self.finishAppendVar();
    }

    // AppendSet appends a Set value into this Column.
    pub fn AppendSet(&mut self, set: types::Set) {
        self.appendNameValue(&set.Name, set.Value);
    }

    // GetNullBitmapCap returns the capacity of nullBitmap.
    pub fn GetNullBitmapCap(&self) -> usize {
        self.nullBitmap.capacity()
    }

    // GetOffsetCap returns the capacity of offsets.
    pub fn GetOffsetCap(&self) -> usize {
        self.offsets.capacity()
    }

    // GetDataCap returns the capacity of data.
    pub fn GetDataCap(&self) -> usize {
        self.data.capacity()
    }

    pub fn typeSize(&self) -> usize {
        if !self.elemBuf.is_empty() {
            return self.elemBuf.len();
        }
        VarElemLen
    }

    // IsFixed returns true when the length of element in column is fixed.
    pub fn IsFixed(&self) -> bool {
        !self.elemBuf.is_empty()
    }

    // same_ref 表达 Go 中 `*Column` 指针相等；在这里中用于 swap/merge 逻辑的可读性。
    pub fn same_ref(&self, other: &Column) -> bool {
        self.reference_id == other.reference_id
    }

    pub(crate) fn reference_root(&self) -> usize {
        self.reference_id
    }

    pub(crate) fn reference_clone(&self) -> Column {
        let mut cloned = self.clone();
        cloned.reference_id = self.reference_id;
        cloned
    }

    // Reset resets this Column according to the EvalType.
    pub fn Reset(&mut self, eType: types::EvalType) {
        match eType {
            types::ETInt => self.ResizeInt64(0, false),
            types::ETReal => self.ResizeFloat64(0, false),
            types::ETDecimal => self.ResizeDecimal(0, false),
            types::ETString => self.ReserveString(0),
            types::ETDatetime | types::ETTimestamp => self.ResizeTime(0, false),
            types::ETDuration => self.ResizeGoDuration(0, false),
            types::ETJson => self.ReserveJSON(0),
            types::ETVectorFloat32 => self.ReserveVectorFloat32(0),
            _ => panic!("invalid EvalType {:?}", eType),
        }
    }

    // Rows returns the row number in current column.
    pub fn Rows(&self) -> usize {
        self.length
    }

    // reset resets the underlying data of this Column but doesn't modify its data type.
    pub fn reset(&mut self) {
        self.length = 0;
        self.nullBitmap.clear();
        if !self.offsets.is_empty() {
            // 第一个 offset 永远是 0，保留下来方便后续切片。
            self.offsets.truncate(1);
        } else if !self.IsFixed() {
            self.offsets.push(0);
        }
        self.data.clear();
    }

    // IsNull returns if this row is null.
    /// 判断指定行是否为 null（bitmap 对应 bit 为 0）。
    pub fn IsNull(&self, rowIdx: usize) -> bool {
        let nullByte = self.nullBitmap[rowIdx / 8];
        nullByte & (1 << (rowIdx & 7)) == 0
    }

    // CopyConstruct copies this Column to dst. If dst is nil, it creates a new Column.
    /// 深拷贝到 `dst`；`dst` 为 `None` 时新建列。
    pub fn CopyConstruct(&self, dst: Option<Box<Column>>) -> Box<Column> {
        if let Some(mut dst) = dst {
            dst.length = self.length;
            dst.nullBitmap.clear();
            dst.nullBitmap.extend_from_slice(&self.nullBitmap);
            dst.offsets.clear();
            dst.offsets.extend_from_slice(&self.offsets);
            dst.data.clear();
            dst.data.extend_from_slice(&self.data);
            dst.elemBuf.clear();
            dst.elemBuf.extend_from_slice(&self.elemBuf);
            return dst;
        }
        Box::new(self.clone())
    }

    // AppendNullBitmap append a null/notnull value to the column's null map.
    pub fn AppendNullBitmap(&mut self, notNull: bool) {
        self.appendNullBitmap(notNull);
    }

    pub fn appendNullBitmap(&mut self, notNull: bool) {
        let idx = self.length >> 3;
        if idx >= self.nullBitmap.len() {
            self.nullBitmap.push(0);
        }
        // 仅在 notNull 时置位；null 保持字节内对应 bit 为 0。
        if notNull {
            let pos = self.length & 7;
            self.nullBitmap[idx] |= 1 << pos;
        }
    }

    // Reserve allocates some memory for the column.
    pub fn Reserve(
        &mut self,
        moreBytesNumNullBitmapNeed: i64,
        moreBytesNumDataNeed: i64,
        moreBytesNumOffsetNeed: i64,
    ) {
        self.nullBitmap
            .reserve(moreBytesNumNullBitmapNeed.max(0) as usize);
        self.data.reserve(moreBytesNumDataNeed.max(0) as usize);
        self.offsets.reserve(moreBytesNumOffsetNeed.max(0) as usize);
    }

    // CalculateLenDeltaForAppendCellNTimesForNullBitMap calculates nullBitmap growth.
    pub fn CalculateLenDeltaForAppendCellNTimesForNullBitMap(&self, times: usize) -> i64 {
        std::cmp::max(
            0,
            (((self.length + times + 7) >> 3) as i64) - self.nullBitmap.len() as i64,
        )
    }

    // CalculateLenDeltaForAppendCellNTimesForFixedElem calculates fixed element memory growth.
    pub fn CalculateLenDeltaForAppendCellNTimesForFixedElem(
        &self,
        src: &Column,
        times: usize,
    ) -> i64 {
        (src.elemBuf.len() * times) as i64
    }

    // CalculateLenDeltaForAppendCellNTimesForVarElem calculates variable element memory growth.
    pub fn CalculateLenDeltaForAppendCellNTimesForVarElem(
        &self,
        src: &Column,
        pos: usize,
        times: usize,
    ) -> i64 {
        (src.offsets[pos + 1] - src.offsets[pos]) * times as i64
    }

    // AppendCellNTimes append the pos-th Cell in source column to target column N times.
    /// 把源列第 `pos` 个单元格追加到本列 `times` 次（含 null bitmap 与 data/offsets）。
    pub fn AppendCellNTimes(&mut self, src: &Column, pos: usize, times: usize) {
        let notNull = !src.IsNull(pos);
        if times == 1 {
            self.appendNullBitmap(notNull);
        } else {
            self.appendMultiSameNullBitmap(notNull, times);
        }
        if self.IsFixed() {
            let elemLen = src.elemBuf.len();
            let offset = pos * elemLen;
            for _ in 0..times {
                self.data
                    .extend_from_slice(&src.data[offset..offset + elemLen]);
            }
        } else {
            let start = src.offsets[pos] as usize;
            let end = src.offsets[pos + 1] as usize;
            for _ in 0..times {
                self.data.extend_from_slice(&src.data[start..end]);
                self.offsets.push(self.data.len() as i64);
            }
        }
        self.length += times;
    }

    // appendMultiSameNullBitmap appends multiple same bit value to nullBitmap.
    pub fn appendMultiSameNullBitmap(&mut self, notNull: bool, num: usize) {
        if num == 0 {
            return;
        }
        let numNewBytes = ((self.length + num + 7) >> 3).saturating_sub(self.nullBitmap.len());
        let b = if notNull { 0xff } else { 0 };
        for _ in 0..numNewBytes {
            self.nullBitmap.push(b);
        }
        if !notNull {
            return;
        }
        // 先把旧 bitmap 最后一个字节剩余 bit 置 1，再清理新 bitmap 最后一个字节冗余 bit。
        let numRemainingBits = self.length % 8;
        let bitMask = (!((1u16 << numRemainingBits) - 1) & 0xff) as u8;
        self.nullBitmap[self.length / 8] |= bitMask;
        let numRedundantBits = self.nullBitmap.len() * 8 - self.length - num;
        let bitMask = ((1u16 << (8 - numRedundantBits)) - 1) as u8;
        let last = self.nullBitmap.len() - 1;
        self.nullBitmap[last] &= bitMask;
    }

    // AppendNNulls append n nulls to the column.
    pub fn AppendNNulls(&mut self, n: usize) {
        self.appendMultiSameNullBitmap(false, n);
        if self.IsFixed() {
            for _ in 0..n {
                self.data.extend_from_slice(&self.elemBuf);
            }
        } else {
            let currentLength = self.offsets[self.length];
            for _ in 0..n {
                self.offsets.push(currentLength);
            }
        }
        self.length += n;
    }

    // AppendNull appends a null value into this Column.
    pub fn AppendNull(&mut self) {
        self.appendNullBitmap(false);
        if self.IsFixed() {
            self.data.extend_from_slice(&self.elemBuf);
        } else {
            self.offsets.push(self.offsets[self.length]);
        }
        self.length += 1;
    }

    /// 定长追加收尾：把 `elemBuf` 写入 data，并标记非 null。
    pub fn finishAppendFixed(&mut self) {
        self.data.extend_from_slice(&self.elemBuf);
        self.appendNullBitmap(true);
        self.length += 1;
    }

    // AppendInt64 appends an int64 value into this Column.
    pub fn AppendInt64(&mut self, i: i64) {
        self.elemBuf = i.to_ne_bytes().to_vec();
        self.finishAppendFixed();
    }

    // AppendUint64 appends a uint64 value into this Column.
    pub fn AppendUint64(&mut self, u: u64) {
        self.elemBuf = u.to_ne_bytes().to_vec();
        self.finishAppendFixed();
    }

    // AppendFloat32 appends a float32 value into this Column.
    pub fn AppendFloat32(&mut self, f: f32) {
        self.elemBuf = f.to_ne_bytes().to_vec();
        self.finishAppendFixed();
    }

    // AppendFloat64 appends a float64 value into this Column.
    pub fn AppendFloat64(&mut self, f: f64) {
        self.elemBuf = f.to_ne_bytes().to_vec();
        self.finishAppendFixed();
    }

    /// 变长追加收尾：标记非 null，并压入当前 data 长度作为新 offset。
    pub fn finishAppendVar(&mut self) {
        self.appendNullBitmap(true);
        self.offsets.push(self.data.len() as i64);
        self.length += 1;
    }

    // AppendString appends a string value into this Column.
    pub fn AppendString(&mut self, str_: &str) {
        self.data.extend_from_slice(str_.as_bytes());
        self.finishAppendVar();
    }

    // AppendBytes appends a byte slice into this Column.
    pub fn AppendBytes(&mut self, b: &[u8]) {
        self.data.extend_from_slice(b);
        self.finishAppendVar();
    }

    // AppendTime appends a time value into this Column.
    pub fn AppendTime(&mut self, t: types::Time) {
        self.elemBuf = t.coreTime.0.to_ne_bytes().to_vec();
        self.finishAppendFixed();
    }

    // AppendEnum appends a Enum value into this Column.
    pub fn AppendEnum(&mut self, enum_: types::Enum) {
        self.appendNameValue(&enum_.Name, enum_.Value);
    }

    // resize resizes the column for fixed-length types.
    /// 将定长列调整为 `n` 个元素；`isNull` 控制 bitmap 全 null 或全非 null。
    pub fn resize(&mut self, n: usize, typeSize: usize, isNull: bool) {
        let sizeData = n * typeSize;
        self.data.resize(sizeData, 0);
        if !isNull {
            // Go 用 emptyBuf 分段 copy 清零；Rust Vec::resize 已经填 0，这里保留分支语义。
            for byte in &mut self.data {
                *byte = 0;
            }
        }

        let mut newNulls = false;
        let sizeNulls = (n + 7) >> 3;
        if self.nullBitmap.capacity() >= sizeNulls {
            self.nullBitmap.resize(sizeNulls, 0);
        } else {
            self.nullBitmap = vec![0; sizeNulls];
            newNulls = true;
        }
        if !isNull || !newNulls {
            let nullVal = if !isNull { 0xFF } else { 0 };
            for item in &mut self.nullBitmap {
                *item = nullVal;
            }
            if n % 8 != 0 && !isNull && !self.nullBitmap.is_empty() {
                let lastByte = (1 << (n % 8)) - 1;
                let last = self.nullBitmap.len() - 1;
                self.nullBitmap[last] = lastByte;
            }
        }

        self.elemBuf.resize(typeSize, 0);
        self.length = n;
    }

    // reserve makes the column capacity enough for n variable-length elements.
    pub fn reserve(&mut self, n: usize, estElemSize: usize) {
        let sizeData = n * estElemSize;
        if self.data.capacity() >= sizeData {
            self.data.clear();
        } else {
            self.data = Vec::with_capacity(sizeData);
        }

        let sizeNulls = (n + 7) >> 3;
        if self.nullBitmap.capacity() >= sizeNulls {
            self.nullBitmap.clear();
        } else {
            self.nullBitmap = Vec::with_capacity(sizeNulls);
        }

        let sizeOffs = n + 1;
        if self.offsets.capacity() >= sizeOffs {
            self.offsets.truncate(1);
        } else {
            self.offsets = Vec::with_capacity(sizeOffs);
            self.offsets.push(0);
        }

        self.elemBuf.clear();
        self.length = 0;
    }

    // SetNull sets the rowIdx to null.
    pub fn SetNull(&mut self, rowIdx: usize, isNull: bool) {
        if isNull {
            self.nullBitmap[rowIdx >> 3] &= !(1 << (rowIdx & 7));
        } else {
            self.nullBitmap[rowIdx >> 3] |= 1 << (rowIdx & 7);
        }
    }

    // SetNulls sets rows in [begin, end) to null.
    /// 批量设置 `[begin, end)` 的 null 位：先对齐到字节边界，再按整字节填充。
    pub fn SetNulls(&mut self, mut begin: usize, end: usize, isNull: bool) {
        let i = ((begin + 7) >> 3) << 3;
        while begin < i && begin < end {
            self.SetNull(begin, isNull);
            begin += 1;
        }
        let v = if !isNull { 0xFF } else { 0 };
        while begin + 8 <= end {
            self.nullBitmap[begin >> 3] = v;
            begin += 8;
        }
        while begin < end {
            self.SetNull(begin, isNull);
            begin += 1;
        }
    }

    // nullCount returns the number of nulls in this Column.
    pub fn nullCount(&self) -> usize {
        let mut cnt = 0;
        let mut i = 0;
        while i + 8 <= self.length {
            cnt += 8 - self.nullBitmap[i >> 3].count_ones() as usize;
            i += 8;
        }
        while i < self.length {
            if self.IsNull(i) {
                cnt += 1;
            }
            i += 1;
        }
        cnt
    }

    // ResizeInt64 resizes the column so that it contains n int64 elements.
    pub fn ResizeInt64(&mut self, n: usize, isNull: bool) {
        self.resize(n, sizeInt64, isNull);
    }

    // ResizeUint64 resizes the column so that it contains n uint64 elements.
    pub fn ResizeUint64(&mut self, n: usize, isNull: bool) {
        self.resize(n, sizeUint64, isNull);
    }

    // ResizeFloat32 resizes the column so that it contains n float32 elements.
    pub fn ResizeFloat32(&mut self, n: usize, isNull: bool) {
        self.resize(n, sizeFloat32, isNull);
    }

    // ResizeFloat64 resizes the column so that it contains n float64 elements.
    pub fn ResizeFloat64(&mut self, n: usize, isNull: bool) {
        self.resize(n, sizeFloat64, isNull);
    }

    // ResizeDecimal resizes the column so that it contains n decimal elements.
    pub fn ResizeDecimal(&mut self, n: usize, isNull: bool) {
        self.resize(n, sizeMyDecimal, isNull);
    }

    // ResizeGoDuration resizes the column so that it contains n duration elements.
    pub fn ResizeGoDuration(&mut self, n: usize, isNull: bool) {
        self.resize(n, sizeGoDuration, isNull);
    }

    // ResizeTime resizes the column so that it contains n Time elements.
    pub fn ResizeTime(&mut self, n: usize, isNull: bool) {
        self.resize(n, sizeTime, isNull);
    }

    // ReserveString changes capacity to store n string elements.
    pub fn ReserveString(&mut self, n: usize) {
        self.reserve(n, 8);
    }

    // ReserveStringWithSizeHint changes capacity using a predetermined size.
    pub fn ReserveStringWithSizeHint(&mut self, n: usize, size: usize) {
        self.reserve(n, size);
    }

    // ReserveBytes changes capacity to store n bytes elements.
    pub fn ReserveBytes(&mut self, n: usize) {
        self.reserve(n, 8);
    }

    // ReserveJSON changes capacity to store n JSON elements.
    pub fn ReserveJSON(&mut self, n: usize) {
        self.reserve(n, 8);
    }

    // ReserveVectorFloat32 changes capacity to store n vectorFloat32 elements.
    pub fn ReserveVectorFloat32(&mut self, n: usize) {
        self.reserve(n, 8);
    }

    // ReserveSet changes capacity to store n set elements.
    pub fn ReserveSet(&mut self, n: usize) {
        self.reserve(n, 8);
    }

    // ReserveEnum changes capacity to store n enum elements.
    pub fn ReserveEnum(&mut self, n: usize) {
        self.reserve(n, 8);
    }

    // Int64s returns an int64 slice stored in this Column.
    pub fn Int64s(&self) -> Vec<i64> {
        chunks_to_vec_i64(&self.data)
    }

    // Uint64s returns a uint64 slice stored in this Column.
    pub fn Uint64s(&self) -> Vec<u64> {
        chunks_to_vec_u64(&self.data)
    }

    // Float32s returns a float32 slice stored in this Column.
    pub fn Float32s(&self) -> Vec<f32> {
        self.data
            .chunks_exact(4)
            .map(|b| f32::from_ne_bytes(b.try_into().unwrap()))
            .collect()
    }

    // Float64s returns a float64 slice stored in this Column.
    pub fn Float64s(&self) -> Vec<f64> {
        self.data
            .chunks_exact(8)
            .map(|b| f64::from_ne_bytes(b.try_into().unwrap()))
            .collect()
    }

    // GoDurations returns a Golang time.Duration slice stored in this Column.
    pub fn GoDurations(&self) -> Vec<i64> {
        chunks_to_vec_i64(&self.data)
    }

    // Decimals returns a MyDecimal slice stored in this Column.
    pub fn Decimals(&self) -> Vec<types::MyDecimal> {
        self.data
            .chunks_exact(types::MyDecimalStructSize)
            .map(decimal_from_bytes)
            .collect()
    }

    // Times returns a Time slice stored in this Column.
    pub fn Times(&self) -> Vec<types::Time> {
        self.data
            .chunks_exact(sizeTime)
            .map(time_from_bytes)
            .collect()
    }

    // GetInt64 returns the int64 in the specific row.
    pub fn GetInt64(&self, rowID: usize) -> i64 {
        read_i64_at(&self.data, rowID * 8)
    }

    // GetUint64 returns the uint64 in the specific row.
    pub fn GetUint64(&self, rowID: usize) -> u64 {
        read_u64_at(&self.data, rowID * 8)
    }

    // GetFloat32 returns the float32 in the specific row.
    pub fn GetFloat32(&self, rowID: usize) -> f32 {
        f32::from_ne_bytes(self.data[rowID * 4..rowID * 4 + 4].try_into().unwrap())
    }

    // GetFloat64 returns the float64 in the specific row.
    pub fn GetFloat64(&self, rowID: usize) -> f64 {
        f64::from_ne_bytes(self.data[rowID * 8..rowID * 8 + 8].try_into().unwrap())
    }

    // GetDecimal returns the decimal in the specific row.
    pub fn GetDecimal(&self, rowID: usize) -> types::MyDecimal {
        let start = rowID * types::MyDecimalStructSize;
        decimal_from_bytes(&self.data[start..start + types::MyDecimalStructSize])
    }

    // GetString returns the string in the specific row.
    pub fn GetString(&self, rowID: usize) -> String {
        String::from_utf8_lossy(self.GetBytes(rowID)).to_string()
    }

    // GetJSON returns the JSON in the specific row.
    pub fn GetJSON(&self, rowID: usize) -> types::BinaryJSON {
        let start = self.offsets[rowID] as usize;
        types::BinaryJSON {
            TypeCode: self.data[start],
            Value: self.data[start + 1..self.offsets[rowID + 1] as usize].to_vec(),
        }
    }

    // GetVectorFloat32 returns the VectorFloat32 in the specific row.
    pub fn GetVectorFloat32(&self, rowID: usize) -> types::VectorFloat32 {
        let data = &self.data[self.offsets[rowID] as usize..self.offsets[rowID + 1] as usize];
        match types::ZeroCopyDeserializeVectorFloat32(data) {
            Ok((v, _)) => v,
            Err(err) => panic!("{:?}", err),
        }
    }

    // GetBytes returns the byte slice in the specific row.
    pub fn GetBytes(&self, rowID: usize) -> &[u8] {
        &self.data[self.offsets[rowID] as usize..self.offsets[rowID + 1] as usize]
    }

    // GetEnum returns the Enum in the specific row.
    pub fn GetEnum(&self, rowID: usize) -> types::Enum {
        let (name, val) = self.getNameValue(rowID);
        types::Enum {
            Name: name,
            Value: val,
        }
    }

    // GetSet returns the Set in the specific row.
    pub fn GetSet(&self, rowID: usize) -> types::Set {
        let (name, val) = self.getNameValue(rowID);
        types::Set {
            Name: name,
            Value: val,
        }
    }

    // GetTime returns the Time in the specific row.
    pub fn GetTime(&self, rowID: usize) -> types::Time {
        time_from_bytes(&self.data[rowID * sizeTime..rowID * sizeTime + sizeTime])
    }

    // GetDuration returns the Duration in the specific row.
    pub fn GetDuration(&self, rowID: usize, fillFsp: i32) -> types::Duration {
        let dur = read_i64_at(&self.data, rowID * 8);
        types::Duration {
            Duration: dur,
            Fsp: fillFsp,
        }
    }

    pub fn getNameValue(&self, rowID: usize) -> (String, u64) {
        let start = self.offsets[rowID] as usize;
        let end = self.offsets[rowID + 1] as usize;
        if start == end {
            return (String::new(), 0);
        }
        let val = read_u64_at(&self.data, start);
        (
            String::from_utf8_lossy(&self.data[start + 8..end]).to_string(),
            val,
        )
    }

    // GetRaw returns the underlying raw bytes in the specific row.
    pub fn GetRaw(&self, rowID: usize) -> &[u8] {
        if self.IsFixed() {
            let elemLen = self.elemBuf.len();
            &self.data[rowID * elemLen..rowID * elemLen + elemLen]
        } else {
            &self.data[self.offsets[rowID] as usize..self.offsets[rowID + 1] as usize]
        }
    }

    // GetRawLength returns the length of the raw.
    pub fn GetRawLength(&self, rowID: usize) -> usize {
        if self.IsFixed() {
            return self.elemBuf.len();
        }
        (self.offsets[rowID + 1] - self.offsets[rowID]) as usize
    }

    // SetRaw sets the raw bytes for the rowIdx-th variable-length element.
    pub fn SetRaw(&mut self, rowID: usize, bs: &[u8]) {
        let start = self.offsets[rowID] as usize;
        let end = self.offsets[rowID + 1] as usize;
        self.data[start..end].copy_from_slice(bs);
    }

    // reconstruct removes all filtered rows according to sel.
    /// 按选择向量 `sel` 原地压缩本列：只保留选中行并重建 bitmap/data/offsets。
    pub fn reconstruct(&mut self, sel: &[usize]) {
        if self.IsFixed() {
            let elemLen = self.elemBuf.len();
            for (dst, src) in sel.iter().enumerate() {
                let idx = dst >> 3;
                let pos = dst & 7;
                if self.IsNull(*src) {
                    self.nullBitmap[idx] &= !(1 << pos);
                } else {
                    let dst_start = dst * elemLen;
                    let src_start = *src * elemLen;
                    self.data
                        .copy_within(src_start..src_start + elemLen, dst_start);
                    self.nullBitmap[idx] |= 1 << pos;
                }
            }
            self.data.truncate(sel.len() * elemLen);
        } else {
            let mut tail = 0usize;
            for (dst, src) in sel.iter().enumerate() {
                let idx = dst >> 3;
                let pos = dst & 7;
                if self.IsNull(*src) {
                    self.nullBitmap[idx] &= !(1 << pos);
                    self.offsets[dst + 1] = tail as i64;
                } else {
                    let start = self.offsets[*src] as usize;
                    let end = self.offsets[*src + 1] as usize;
                    self.data.copy_within(start..end, tail);
                    tail += end - start;
                    self.offsets[dst + 1] = tail as i64;
                    self.nullBitmap[idx] |= 1 << pos;
                }
            }
            self.data.truncate(tail);
            self.offsets.truncate(sel.len() + 1);
        }
        self.length = sel.len();

        // clean nullBitmap: 清掉 selection 后已不存在行的 bitmap 尾部。
        self.nullBitmap.truncate((sel.len() + 7) >> 3);
        let idx = sel.len() >> 3;
        if idx < self.nullBitmap.len() {
            let pos = sel.len() & 7;
            self.nullBitmap[idx] &= (1 << pos) - 1;
        }
    }

    // CopyReconstruct copies this Column to dst and removes unselected rows.
    /// 按 `sel` 过滤复制到 `dst`；无选择或选择已是升序全集时退化为 `CopyConstruct`。
    pub fn CopyReconstruct(&self, sel: Option<&[usize]>, dst: Option<Box<Column>>) -> Box<Column> {
        if sel.is_none() {
            return self.CopyConstruct(dst);
        }
        let sel = sel.unwrap();
        if sel.len() == self.length {
            let mut ascend = true;
            for i in 1..sel.len() {
                if sel[i] < sel[i - 1] {
                    ascend = false;
                    break;
                }
            }
            if ascend {
                return self.CopyConstruct(dst);
            }
        }

        let mut dst = match dst {
            Some(mut col) => {
                col.reset();
                col
            }
            None => newColumn(self.typeSize(), sel.len()),
        };

        if self.IsFixed() {
            let elemLen = self.elemBuf.len();
            dst.elemBuf = vec![0; elemLen];
            for i in sel {
                dst.appendNullBitmap(!self.IsNull(*i));
                dst.data
                    .extend_from_slice(&self.data[*i * elemLen..*i * elemLen + elemLen]);
                dst.length += 1;
            }
        } else {
            dst.elemBuf.clear();
            if dst.offsets.is_empty() {
                dst.offsets.push(0);
            }
            for i in sel {
                dst.appendNullBitmap(!self.IsNull(*i));
                let start = self.offsets[*i] as usize;
                let end = self.offsets[*i + 1] as usize;
                dst.data.extend_from_slice(&self.data[start..end]);
                dst.offsets.push(dst.data.len() as i64);
                dst.length += 1;
            }
        }
        dst
    }

    // MergeNulls merges these columns' null bitmaps.
    /// 与其它列的 null bitmap 做按位与（任一侧为 null 则结果为 null）；要求定长且行数一致。
    pub fn MergeNulls(&mut self, cols: &[Column]) {
        if !self.IsFixed() {
            panic!("result column should be fixed-length type");
        }
        for col in cols {
            if self.length != col.length {
                panic!(
                    "should ensure all columns have the same length, expect {}, but got {}",
                    self.length, col.length
                );
            }
        }
        for col in cols {
            for i in 0..self.nullBitmap.len() {
                // bit 0 is null and 1 is not null, so AND operations implement “any null -> null”.
                self.nullBitmap[i] &= col.nullBitmap[i];
            }
        }
    }

    // DestroyDataForTest destroys data in the column for deep-copy tests.
    pub fn DestroyDataForTest(&mut self) {
        let dataByteNum = self.data.len();
        for i in 0..dataByteNum {
            self.data[i] = rand::random::<u8>();
        }
    }

    // ContainsVeryLargeElement checks if any element length is greater than math.MaxUint32.
    pub fn ContainsVeryLargeElement(&self) -> bool {
        if self.length == 0 {
            return false;
        }
        if self.IsFixed() {
            return false;
        }
        if self.offsets[self.length] <= u32::MAX as i64 {
            return false;
        }
        for i in 0..self.length {
            if self.offsets[i + 1] - self.offsets[i] > u32::MAX as i64 {
                return true;
            }
        }
        false
    }
}

/// 定长类型元素字节宽度常量（与 Go 侧 sizeXxx 对齐）。
pub const sizeInt64: usize = std::mem::size_of::<i64>();
pub const sizeUint64: usize = std::mem::size_of::<u64>();
pub const sizeUint32: usize = std::mem::size_of::<u32>();
pub const sizeFloat32: usize = std::mem::size_of::<f32>();
pub const sizeFloat64: usize = std::mem::size_of::<f64>();
pub const sizeMyDecimal: usize = types::MyDecimalStructSize;
pub const sizeGoDuration: usize = std::mem::size_of::<i64>();
pub const sizeTime: usize = std::mem::size_of::<types::Time>();

/// 用于批量清零的静态零缓冲（对齐 Go `emptyBuf`）。
pub static emptyBuf: [u8; 4 * 1024] = [0; 4 * 1024];

/// 按本机字节序从 `data` 偏移处读取 i64。
fn read_i64_at(data: &[u8], offset: usize) -> i64 {
    i64::from_ne_bytes(data[offset..offset + 8].try_into().unwrap())
}

/// 按本机字节序从 `data` 偏移处读取 u64。
fn read_u64_at(data: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(data[offset..offset + 8].try_into().unwrap())
}

/// 把连续字节切成 i64 向量。
fn chunks_to_vec_i64(data: &[u8]) -> Vec<i64> {
    data.chunks_exact(8)
        .map(|b| i64::from_ne_bytes(b.try_into().unwrap()))
        .collect()
}

/// 把连续字节切成 u64 向量。
fn chunks_to_vec_u64(data: &[u8]) -> Vec<u64> {
    data.chunks_exact(8)
        .map(|b| u64::from_ne_bytes(b.try_into().unwrap()))
        .collect()
}

/// 将 `MyDecimal` 序列化为固定长度字节布局。
fn decimal_to_bytes(decimal: &types::MyDecimal) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(types::MyDecimalStructSize);
    bytes.push(decimal.digitsInt as u8);
    bytes.push(decimal.digitsFrac as u8);
    bytes.push(decimal.resultFrac as u8);
    bytes.push(u8::from(decimal.negative));
    for word in decimal.wordBuf {
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    debug_assert_eq!(bytes.len(), types::MyDecimalStructSize);
    bytes
}

/// 从固定布局字节反序列化为 `MyDecimal`。
fn decimal_from_bytes(bytes: &[u8]) -> types::MyDecimal {
    assert_eq!(bytes.len(), types::MyDecimalStructSize);
    let mut word_buf = [0i32; 9];
    for (index, word) in bytes[4..].chunks_exact(4).enumerate() {
        word_buf[index] = i32::from_ne_bytes(word.try_into().unwrap());
    }
    types::MyDecimal {
        digitsInt: bytes[0] as i8,
        digitsFrac: bytes[1] as i8,
        resultFrac: bytes[2] as i8,
        negative: bytes[3] != 0,
        wordBuf: word_buf,
    }
}

/// 从本机序字节还原 `Time`。
fn time_from_bytes(bytes: &[u8]) -> types::Time {
    assert_eq!(bytes.len(), sizeTime);
    types::Time {
        coreTime: types::CoreTime(u64::from_ne_bytes(bytes.try_into().unwrap())),
    }
}
