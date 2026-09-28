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

// Chunk/Column 的二进制编解码，以及 Decoder 对中间 Chunk 的增量行拷贝。
//
// Codec 把各列序列化为 length/nullCount/nullBitmap/offsets/data；
// Decoder 对接 coprocessor 返回数据，按 requiredRows（上取整到 8 的倍数）分批填入目标 Chunk。

// Chunk/Column 的二进制编码、解码，以及 Decoder 复用中间 Chunk 的增量拷贝流程。

// Codec is used to encode a Chunk to bytes and decode a Chunk from bytes.
/// 按列类型对 Chunk 做字节编解码。
pub struct Codec {
    // colTypes is used only for decoding to determine fixed element width.
    pub colTypes: Vec<types::FieldType>,
}

// NewCodec creates a new Codec object for encode or decode a Chunk.
/// 创建 Codec；`colTypes` 解码时用于判定固定元素宽度。
pub fn NewCodec(colTypes: Vec<types::FieldType>) -> Box<Codec> {
    Box::new(Codec { colTypes })
}

impl Codec {
    // Encode encodes a Chunk to a byte slice.
    /// 将 Chunk 各列编码为连续字节。
    pub fn Encode(&self, chk: &Chunk) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(chk.MemoryUsage() as usize);
        for col in &chk.columns {
            buffer = self.encodeColumn(buffer, col);
        }
        buffer
    }

    /// 编码单列并追加到 buffer。
    pub fn encodeColumn(&self, mut buffer: Vec<u8>, col: &Column) -> Vec<u8> {
        // Go 使用 binary.LittleEndian 写 length/nullCount，后续字段直接拼接原始字节。
        buffer.extend_from_slice(&(col.length as u32).to_le_bytes());
        buffer.extend_from_slice(&(col.nullCount() as u32).to_le_bytes());

        if col.nullCount() > 0 {
            let numNullBitmapBytes = (col.length + 7) / 8;
            buffer.extend_from_slice(&col.nullBitmap[..numNullBitmapBytes]);
        }

        if !col.IsFixed() {
            let numOffsetBytes = (col.length + 1) * 8;
            let offsetBytes = i64SliceToBytes(&col.offsets);
            buffer.extend_from_slice(&offsetBytes[..numOffsetBytes]);
        }

        buffer.extend_from_slice(&col.data);
        buffer
    }

    // Decode decodes a Chunk from a byte slice, return the remained unused bytes.
    /// 从字节流解码出新 Chunk，并返回剩余未消费字节。
    pub fn Decode<'a>(&self, mut buffer: &'a [u8]) -> (Box<Chunk>, &'a [u8]) {
        let mut chk = Box::new(Chunk {
            sel: None,
            columns: Vec::new(),
            numVirtualRows: 0,
            capacity: 0,
            requiredRows: 0,
            inCompleteChunk: false,
        });
        let mut ordinal = 0;
        while !buffer.is_empty() {
            let mut col = Column::default();
            buffer = self.decodeColumn(buffer, &mut col, ordinal);
            chk.columns.push(col);
            ordinal += 1;
        }
        (chk, buffer)
    }

    // DecodeToChunk decodes a Chunk from a byte slice, return the remained unused bytes.
    /// 解码到已有 Chunk 的各列，返回剩余字节。
    pub fn DecodeToChunk<'a>(&self, mut buffer: &'a [u8], chk: &mut Chunk) -> &'a [u8] {
        for i in 0..chk.columns.len() {
            buffer = self.decodeColumn(buffer, &mut chk.columns[i], i);
        }
        buffer
    }

    // decodeColumn decodes a Column from a byte slice, return the remained unused bytes.
    /// 解码单列到 `col`；固定/变长分支由 `colTypes[ordinal]` 决定。
    pub fn decodeColumn<'a>(
        &self,
        mut buffer: &'a [u8],
        col: &mut Column,
        ordinal: usize,
    ) -> &'a [u8] {
        // Todo(Shenghui Wu): Optimize all data is null.
        col.length = u32::from_le_bytes(buffer[..4].try_into().unwrap()) as usize;
        buffer = &buffer[4..];

        let nullCount = u32::from_le_bytes(buffer[..4].try_into().unwrap()) as usize;
        buffer = &buffer[4..];

        if nullCount > 0 {
            let numNullBitmapBytes = (col.length + 7) / 8;
            col.nullBitmap = buffer[..numNullBitmapBytes].to_vec();
            buffer = &buffer[numNullBitmapBytes..];
        } else {
            self.setAllNotNull(col);
        }

        let numFixedBytes = getFixedLen(&self.colTypes[ordinal]);
        let numDataBytes;
        if numFixedBytes == VarElemLen {
            let numOffsetBytes = (col.length + 1) * 8;
            col.offsets = bytesToI64Slice(&buffer[..numOffsetBytes]);
            buffer = &buffer[numOffsetBytes..];
            numDataBytes = col.offsets[col.length] as usize;
        } else {
            numDataBytes = numFixedBytes * col.length;
            if col.elemBuf.capacity() < numFixedBytes {
                col.elemBuf = vec![0; numFixedBytes];
            }
        }

        col.data = buffer[..numDataBytes].to_vec();
        // Go 标记 avoidReusing，避免 Column 复用时持有 gRPC 响应底层内存；保留该资源语义。
        col.avoidReusing = true;
        &buffer[numDataBytes..]
    }

    /// 当 nullCount 为 0 时，用全 1 bitmap 填充 nullBitmap。
    pub fn setAllNotNull(&self, col: &mut Column) {
        let numNullBitmapBytes = (col.length + 7) / 8;
        col.nullBitmap.clear();
        let mut i = 0;
        while i < numNullBitmapBytes {
            let numAppendBytes = std::cmp::min(numNullBitmapBytes - i, allNotNullBitmap.len());
            col.nullBitmap
                .extend_from_slice(&allNotNullBitmap[..numAppendBytes]);
            i += numAppendBytes;
        }
    }
}

/// 将 `i64` 切片按本机字节序展开为字节。
pub fn i64SliceToBytes(i64s: &[i64]) -> Vec<u8> {
    if i64s.is_empty() {
        return Vec::new();
    }
    let mut b = Vec::with_capacity(i64s.len() * 8);
    for item in i64s {
        b.extend_from_slice(&item.to_ne_bytes());
    }
    b
}

/// 将字节按本机字节序还原为 `i64` 切片。
pub fn bytesToI64Slice(b: &[u8]) -> Vec<i64> {
    if b.is_empty() {
        return Vec::new();
    }
    let mut i64s = Vec::with_capacity(b.len() / 8);
    for chunk in b.chunks_exact(8) {
        i64s.push(i64::from_ne_bytes(chunk.try_into().unwrap()));
    }
    i64s
}

/// 预置的全非空 bitmap 模板，避免逐字节填充。
pub static allNotNullBitmap: [u8; 128] = [0xFF; 128];

// VarElemLen indicates this Column is a variable length Column.
/// 变长列标记：`getFixedLen` 对非固定类型返回该值。
pub const VarElemLen: usize = usize::MAX;

/// 返回固定类型元素字节宽，变长类型返回 `VarElemLen`。
pub fn getFixedLen(colType: &types::FieldType) -> usize {
    match colType.GetType() {
        mysql::TypeFloat => 4,
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong
        | mysql::TypeDouble
        | mysql::TypeYear
        | mysql::TypeDuration => 8,
        mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => sizeTime,
        mysql::TypeNewDecimal => types::MyDecimalStructSize,
        _ => VarElemLen,
    }
}

// GetFixedLen get the memory size of a fixed-length type.
/// `getFixedLen` 的公开别名。
pub fn GetFixedLen(colType: &types::FieldType) -> usize {
    getFixedLen(colType)
}

// EstimateTypeWidth estimates the average width of values of the type.
/// 估算类型平均宽度，用于内存预分配；超大 varchar 用经验上限。
pub fn EstimateTypeWidth(colType: &types::FieldType) -> usize {
    let mut colLen = getFixedLen(colType);
    // Fixed-width type is easy: its average width is the fixed width itself.
    if colLen != VarElemLen {
        return colLen;
    }

    let declared_len = colType.GetFlen();
    if declared_len > 0 {
        colLen = declared_len as usize;
        if colLen <= 32 {
            return colLen;
        }
        if colLen < 1000 {
            return 32 + (colLen - 32) / 2;
        }
        // Go 借鉴 PostgreSQL 估算：超大 varchar 取一个固定经验宽度，避免按声明上限膨胀。
        return 32 + (1000 - 32) / 2;
    }
    32
}

// Decoder decodes data returned from the coprocessor and stores the result in Chunk.
/// 解码 coprocessor 返回数据到中间 Chunk，再按需拷到输出 Chunk。
pub struct Decoder {
    pub intermChk: Box<Chunk>,
    pub codec: Box<Codec>,
    pub remainedRows: usize,
}

// NewDecoder creates a new Decoder object for decode a Chunk.
/// 创建 Decoder，`chk` 作为可复用的中间缓冲。
pub fn NewDecoder(chk: Box<Chunk>, colTypes: Vec<types::FieldType>) -> Box<Decoder> {
    Box::new(Decoder {
        intermChk: chk,
        codec: NewCodec(colTypes),
        remainedRows: 0,
    })
}

impl Decoder {
    // Decode decodes multiple rows of Decoder.intermChk and stores the result in chk.
    /// 从中间 Chunk 向目标追加一批行（行数上取整到 8 的倍数）。
    pub fn Decode(&mut self, chk: &mut Chunk) {
        let mut requiredRows = chk.RequiredRows() - chk.NumRows();
        // Go 把 requiredRows 向上取到 8 的倍数，以减少 nullBitmap 拷贝时的移位成本。
        requiredRows = std::cmp::min(((requiredRows + 7) >> 3) << 3, self.remainedRows);
        for i in 0..chk.NumCols() {
            self.decodeColumn(chk, i, requiredRows);
        }
        self.remainedRows -= requiredRows;
    }

    // Reset decodes data and store the result in Decoder.intermChk.
    /// 用新字节重置中间 Chunk，并记录剩余行数。
    pub fn Reset(&mut self, data: &[u8]) {
        self.codec.DecodeToChunk(data, &mut self.intermChk);
        self.remainedRows = self.intermChk.NumRows();
    }

    // IsFinished indicates whether Decoder.intermChk has been dried up.
    /// 中间 Chunk 是否已消费完。
    pub fn IsFinished(&self) -> bool {
        self.remainedRows == 0
    }

    // RemainedRows indicates Decoder.intermChk has remained rows.
    /// 中间 Chunk 尚未拷出的行数。
    pub fn RemainedRows(&self) -> usize {
        self.remainedRows
    }

    // ReuseIntermChk swaps Decoder.intermChk with chk directly when enough rows remain.
    /// 剩余行足够时直接交换中间列，避免逐行拷贝。
    pub fn ReuseIntermChk(&mut self, chk: &mut Chunk) {
        for (i, col) in self.intermChk.columns.iter_mut().enumerate() {
            col.length = self.remainedRows;
            let elemLen = getFixedLen(&self.codec.colTypes[i]);
            if elemLen == VarElemLen {
                // 变长列复用前要把 offsets 调整为从 0 开始，和 Go 中 deltaOffset 语义一致。
                let deltaOffset = col.offsets[0];
                if deltaOffset != 0 {
                    for offset in &mut col.offsets {
                        *offset -= deltaOffset;
                    }
                }
            }
        }
        chk.SwapColumns(&mut self.intermChk);
        self.remainedRows = 0;
    }

    /// 把中间列的前 `requiredRows` 行追加到目标列，并裁剪源列前缀。
    pub fn decodeColumn(&mut self, chk: &mut Chunk, ordinal: usize, requiredRows: usize) {
        let elemLen = getFixedLen(&self.codec.colTypes[ordinal]);
        let mut numDataBytes = elemLen * requiredRows;
        let srcCol = &mut self.intermChk.columns[ordinal];
        let destCol = &mut chk.columns[ordinal];

        if elemLen == VarElemLen {
            // 变长列追加 offsets 后，必须用 destCol 当前尾 offset 修正新 offsets。
            numDataBytes = (srcCol.offsets[requiredRows] - srcCol.offsets[0]) as usize;
            let deltaOffset = destCol.offsets[destCol.length] - srcCol.offsets[0];
            destCol
                .offsets
                .extend_from_slice(&srcCol.offsets[1..requiredRows + 1]);
            for i in destCol.length + 1..=destCol.length + requiredRows {
                destCol.offsets[i] += deltaOffset;
            }
            srcCol.offsets = srcCol.offsets[requiredRows..].to_vec();
        }

        let numNullBitmapBytes = (requiredRows + 7) >> 3;
        if destCol.length % 8 == 0 {
            destCol
                .nullBitmap
                .extend_from_slice(&srcCol.nullBitmap[..numNullBitmapBytes]);
        } else {
            destCol.appendMultiSameNullBitmap(false, requiredRows);
            let bitMapLen = destCol.nullBitmap.len();
            let bitOffset = destCol.length % 8;
            let startIdx = (destCol.length - 1) >> 3;
            for i in 0..numNullBitmapBytes {
                destCol.nullBitmap[startIdx + i] |= srcCol.nullBitmap[i] << bitOffset;
                if startIdx + i + 1 < bitMapLen {
                    destCol.nullBitmap[startIdx + i + 1] |= srcCol.nullBitmap[i] >> (8 - bitOffset);
                }
            }
        }
        // 清理最后一个字节中超出有效行数的冗余 bit，保持 Go 的 bitmap 规范。
        let numRedundantBits = destCol.nullBitmap.len() * 8 - destCol.length - requiredRows;
        let bitMask = ((1u16 << (8 - numRedundantBits)) - 1) as u8;
        let last = destCol.nullBitmap.len() - 1;
        destCol.nullBitmap[last] &= bitMask;

        srcCol.nullBitmap = srcCol.nullBitmap[numNullBitmapBytes..].to_vec();
        destCol.length += requiredRows;
        destCol.data.extend_from_slice(&srcCol.data[..numDataBytes]);
        srcCol.data = srcCol.data[numDataBytes..].to_vec();
    }
}
