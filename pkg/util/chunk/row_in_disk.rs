// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 按行格式将 Chunk 溢写到临时磁盘，并支持按行/按块随机读回。
//
// 对应 Go `row_in_disk.go`。布局为双文件：数据文件存每行序列化内容，
// 偏移文件存每行绝对偏移（小端 i64），从而可在随机读之后继续追加。
// Spill（溢写）用于内存不足时把算子中间结果落到磁盘。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use crate::{Chunk, ChunkError, Result, Row, RowPtr, disk, memory, types};

/// 数据临时文件名前缀。
pub const defaultChunkDataInDiskByRowsPath: &str = "chunk.DataInDiskByRows";
/// 行偏移临时文件名前缀。
pub const defaultChunkDataInDiskByRowsOffsetPath: &str = "chunk.DataInDiskByRowsOffset";

/// 按行把 Chunk 写入磁盘：数据文件 + 偏移文件双文件布局，支持追加与随机读。
/// DataInDiskByRows stores every Chunk in row format and keeps a second file
/// containing the absolute offset of each row. This is the same two-file layout
/// as row_in_disk.go and permits appending after random reads.
pub struct DataInDiskByRows {
    /// 列类型元数据，决定每行如何序列化/反序列化。
    fieldTypes: Vec<types::FieldType>,
    /// 每个已写入 Chunk 的行数。
    numRowsOfEachChunk: Vec<usize>,
    /// 每个 Chunk 第一行在全局行序中的起始下标。
    rowNumOfEachChunkFirstRow: Vec<usize>,
    /// 已写入的总行数。
    totalNumRows: usize,
    /// 磁盘用量追踪器。
    diskTracker: Arc<memory::Tracker>,
    /// 行数据临时文件。
    dataFile: Option<tempfile::NamedTempFile>,
    /// 行偏移临时文件。
    offsetFile: Option<tempfile::NamedTempFile>,
    /// 数据文件当前写偏移。
    dataOffWrite: i64,
    /// 偏移文件当前写偏移。
    offsetOffWrite: i64,
}

impl DataInDiskByRows {
    /// 构造空的磁盘行容器，尚未打开临时文件。
    pub fn New(fieldTypes: Vec<types::FieldType>) -> Self {
        Self {
            fieldTypes,
            numRowsOfEachChunk: Vec::new(),
            rowNumOfEachChunkFirstRow: Vec::new(),
            totalNumRows: 0,
            diskTracker: Arc::from(disk::NewTracker(memory::LabelForChunkDataInDiskByRows, -1)),
            dataFile: None,
            offsetFile: None,
            dataOffWrite: 0,
            offsetOffWrite: 0,
        }
    }

    /// 惰性创建数据与偏移两个临时文件。
    fn initDiskFile(&mut self) -> Result<()> {
        if self.dataFile.is_none() {
            self.dataFile = Some(
                tempfile::Builder::new()
                    .prefix(defaultChunkDataInDiskByRowsPath)
                    .tempfile()?,
            );
            self.offsetFile = Some(
                tempfile::Builder::new()
                    .prefix(defaultChunkDataInDiskByRowsOffsetPath)
                    .tempfile()?,
            );
        }
        Ok(())
    }

    /// 返回已写入的总行数。
    pub fn Len(&self) -> usize {
        self.totalNumRows
    }
    /// 返回磁盘用量 Tracker。
    pub fn GetDiskTracker(&self) -> Arc<memory::Tracker> {
        self.diskTracker.clone()
    }

    /// 将非空 Chunk 按行序列化追加到磁盘，并更新偏移索引。
    pub fn Add(&mut self, chunk: &Chunk) -> Result<()> {
        if chunk.NumRows() == 0 {
            return Err(ChunkError::Message(
                "chunk appended to List should have at least 1 row".to_owned(),
            ));
        }
        if chunk.NumCols() != self.fieldTypes.len() {
            return Err(ChunkError::Message(
                "chunk has different field count".to_owned(),
            ));
        }
        self.initDiskFile()?;

        // 先写数据文件，再把各行绝对偏移写入偏移文件。
        let mut diskChunk = chunkInDisk::new(chunk.clone(), self.dataOffWrite);
        let written = {
            let file = self
                .dataFile
                .as_mut()
                .expect("initialized data file")
                .as_file_mut();
            file.seek(SeekFrom::Start(self.dataOffWrite as u64))?;
            let written = diskChunk.WriteTo(file)?;
            file.flush()?;
            written
        };
        self.dataOffWrite += written;

        let offsets = diskChunk.getOffsetsOfRows();
        let offsetWritten = {
            let file = self
                .offsetFile
                .as_mut()
                .expect("initialized offset file")
                .as_file_mut();
            file.seek(SeekFrom::Start(self.offsetOffWrite as u64))?;
            let written = offsets.WriteTo(file)?;
            file.flush()?;
            written
        };
        self.offsetOffWrite += offsetWritten;

        self.numRowsOfEachChunk.push(chunk.NumRows());
        self.rowNumOfEachChunkFirstRow.push(self.totalNumRows);
        self.totalNumRows += chunk.NumRows();
        self.diskTracker.Consume(written + offsetWritten);
        Ok(())
    }

    /// 按 Chunk 下标整块读回并重建为内存 Chunk。
    pub fn GetChunk(&self, chkIdx: usize) -> Result<Chunk> {
        let rowCount = self.NumRowsOfChunk(chkIdx);
        let mut chunk = *crate::New(self.fieldTypes.clone(), rowCount, rowCount);
        let firstOffset = self.getOffset(chkIdx as u32, 0)?;
        let mut reader = self.dataReader(firstOffset)?;
        for _ in 0..rowCount {
            let mut format = rowInDisk::new(self.fieldTypes.len());
            format.ReadFrom(&mut reader)?;
            chunk = format.diskFormatRow.toRow(&self.fieldTypes, Some(chunk)).1;
        }
        Ok(chunk)
    }

    /// 按 `RowPtr` 读回单行（内部新建 Chunk 承载）。
    pub fn GetRow(&self, ptr: RowPtr) -> Result<Row> {
        self.GetRowAndAppendToChunk(ptr, None)
            .map(|(row, chunk)| Row::from_owned(chunk, row.idx))
    }

    /// 按 `RowPtr` 读回单行，可选追加到已有未满 Chunk 以复用缓冲。
    pub fn GetRowAndAppendToChunk(
        &self,
        ptr: RowPtr,
        chunk: Option<Chunk>,
    ) -> Result<(Row, Chunk)> {
        if ptr.ChkIdx as usize >= self.NumChunks()
            || ptr.RowIdx as usize >= self.NumRowsOfChunk(ptr.ChkIdx as usize)
        {
            return Err(ChunkError::Message("row pointer out of range".to_owned()));
        }
        let offset = self.getOffset(ptr.ChkIdx, ptr.RowIdx)?;
        let mut reader = self.dataReader(offset)?;
        let mut format = rowInDisk::new(self.fieldTypes.len());
        format.ReadFrom(&mut reader)?;
        Ok(format.diskFormatRow.toRow(&self.fieldTypes, chunk))
    }

    /// 从数据文件指定偏移打开只读句柄。
    fn dataReader(&self, offset: i64) -> Result<File> {
        let mut reader = self
            .dataFile
            .as_ref()
            .ok_or_else(|| ChunkError::Message("disk data is empty".to_owned()))?
            .reopen()?;
        reader.seek(SeekFrom::Start(offset as u64))?;
        Ok(reader)
    }

    /// 从偏移文件读取指定 (chunk, row) 在数据文件中的绝对偏移。
    fn getOffset(&self, chkIdx: u32, rowIdx: u32) -> Result<i64> {
        let ordinal = self.rowNumOfEachChunkFirstRow[chkIdx as usize] + rowIdx as usize;
        let mut file = self
            .offsetFile
            .as_ref()
            .ok_or_else(|| ChunkError::Message("offset file is empty".to_owned()))?
            .reopen()?;
        // 每个偏移占 8 字节小端 i64。
        file.seek(SeekFrom::Start((ordinal * 8) as u64))?;
        let mut bytes = [0_u8; 8];
        file.read_exact(&mut bytes).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                ChunkError::Message(
                    "The file spilled is broken, can not get data offset from the disk".to_owned(),
                )
            } else {
                error.into()
            }
        })?;
        Ok(i64::from_le_bytes(bytes))
    }

    /// 返回指定 Chunk 的行数。
    pub fn NumRowsOfChunk(&self, chkID: usize) -> usize {
        self.numRowsOfEachChunk[chkID]
    }
    /// 返回已写入的 Chunk 个数。
    pub fn NumChunks(&self) -> usize {
        self.numRowsOfEachChunk.len()
    }

    /// 关闭并丢弃临时文件，清零磁盘 Tracker。
    pub fn Close(&mut self) -> Result<()> {
        self.dataFile.take();
        self.offsetFile.take();
        self.diskTracker.Consume(-self.diskTracker.BytesConsumed());
        self.dataOffWrite = 0;
        self.offsetOffWrite = 0;
        Ok(())
    }
}

impl Drop for DataInDiskByRows {
    fn drop(&mut self) {
        let _ = self.Close();
    }
}

/// 单个 Chunk 在写入磁盘前的中间表示：持有行偏移列表。
pub struct chunkInDisk {
    Chunk: Chunk,
    /// 写入起始的数据文件绝对偏移。
    offWrite: i64,
    offsetsOfRows: offsetsOfRows,
}

impl chunkInDisk {
    fn new(chunk: Chunk, offWrite: i64) -> Self {
        Self {
            Chunk: chunk,
            offWrite,
            offsetsOfRows: offsetsOfRows::default(),
        }
    }

    /// 逐行序列化写入，并记录每行相对 `offWrite` 的绝对偏移。
    pub fn WriteTo<W: Write>(&mut self, writer: &mut W) -> Result<i64> {
        let mut written = 0_i64;
        let mut reuse = None;
        self.offsetsOfRows.0.clear();
        self.offsetsOfRows.0.reserve(self.Chunk.NumRows());
        for rowIdx in 0..self.Chunk.NumRows() {
            let format = convertFromRow(self.Chunk.GetRow(rowIdx), reuse.take());
            self.offsetsOfRows.0.push(self.offWrite + written);
            written += rowInDisk {
                numCol: 0,
                diskFormatRow: format.clone(),
            }
            .WriteTo(writer)?;
            reuse = Some(format);
        }
        Ok(written)
    }

    /// 返回本 Chunk 内各行在数据文件中的绝对偏移列表。
    pub fn getOffsetsOfRows(&self) -> offsetsOfRows {
        self.offsetsOfRows.clone()
    }
}

#[derive(Clone, Default)]
/// 一行偏移序列，序列化为连续小端 i64。
pub struct offsetsOfRows(Vec<i64>);

impl offsetsOfRows {
    /// 将全部偏移以小端 i64 写入。
    pub fn WriteTo<W: Write>(&self, writer: &mut W) -> Result<i64> {
        let mut written = 0;
        for offset in &self.0 {
            writer.write_all(&offset.to_le_bytes())?;
            written += 8;
        }
        Ok(written)
    }
}

#[derive(Clone, Default)]
/// 磁盘上行记录的读写包装：列大小头 + cell 载荷。
pub struct rowInDisk {
    numCol: usize,
    diskFormatRow: diskFormatRow,
}

impl rowInDisk {
    fn new(numCol: usize) -> Self {
        Self {
            numCol,
            diskFormatRow: diskFormatRow::default(),
        }
    }

    /// 写出：先写每列 size（i64），再写非 null cell 字节。
    pub fn WriteTo<W: Write>(&self, writer: &mut W) -> Result<i64> {
        let mut written = 0_i64;
        for size in &self.diskFormatRow.sizesOfColumns {
            writer.write_all(&size.to_le_bytes())?;
            written += 8;
        }
        for cell in &self.diskFormatRow.cells {
            writer.write_all(cell)?;
            written += cell.len() as i64;
        }
        Ok(written)
    }

    /// 读入：按 `numCol` 读取 size 头，size=-1 表示 NULL，否则读对应长度 cell。
    pub fn ReadFrom<R: Read>(&mut self, reader: &mut R) -> Result<i64> {
        let mut read = 0_i64;
        self.diskFormatRow.sizesOfColumns.clear();
        self.diskFormatRow.cells.clear();
        for _ in 0..self.numCol {
            let mut bytes = [0_u8; 8];
            reader.read_exact(&mut bytes)?;
            read += 8;
            self.diskFormatRow
                .sizesOfColumns
                .push(i64::from_le_bytes(bytes));
        }
        for size in &self.diskFormatRow.sizesOfColumns {
            if *size == -1 {
                continue;
            }
            if *size < -1 {
                return Err(ChunkError::Message(
                    "negative cell size in spilled row".to_owned(),
                ));
            }
            let mut cell = vec![0_u8; *size as usize];
            reader.read_exact(&mut cell)?;
            read += *size;
            self.diskFormatRow.cells.push(cell);
        }
        Ok(read)
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq)]
/// 行在磁盘上的逻辑格式：每列长度（-1=NULL）与非空 cell 字节序列。
pub struct diskFormatRow {
    pub sizesOfColumns: Vec<i64>,
    pub cells: Vec<Vec<u8>>,
}

/// 将内存 `Row` 转为磁盘格式；可复用上一行的缓冲以减少分配。
pub fn convertFromRow(row: Row, reuse: Option<diskFormatRow>) -> diskFormatRow {
    let mut format = reuse.unwrap_or_else(|| diskFormatRow {
        sizesOfColumns: Vec::with_capacity(row.Len()),
        cells: Vec::with_capacity(row.Len()),
    });
    format.sizesOfColumns.clear();
    format.cells.clear();
    for colIdx in 0..row.Len() {
        if row.IsNull(colIdx) {
            format.sizesOfColumns.push(-1);
        } else {
            let cell = row.GetRaw(colIdx);
            format.sizesOfColumns.push(cell.len() as i64);
            format.cells.push(cell);
        }
    }
    format
}

impl diskFormatRow {
    /// 将磁盘格式行追加到 Chunk（可复用未满 Chunk），返回新行与承载 Chunk。
    pub fn toRow(&self, fields: &[types::FieldType], chunk: Option<Chunk>) -> (Row, Chunk) {
        let mut chunk = match chunk {
            Some(chunk) if !chunk.IsFull() => chunk,
            _ => *crate::New(fields.to_vec(), 1024, 1024),
        };
        let mut cellOffset = 0;
        for (colIdx, size) in self.sizesOfColumns.iter().enumerate() {
            if *size == -1 {
                chunk.AppendNull(colIdx);
            } else {
                chunk.AppendRaw(colIdx, &self.cells[cellOffset]);
                cellOffset += 1;
            }
        }
        (chunk.GetRow(chunk.NumRows() - 1), chunk)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// `read_at` 失败分类：到达末尾或其它错误。
pub enum ReadAtError {
    Eof,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 一次 `read_at` 的结果：已读字节数与可选错误。
pub struct ReadAtResult {
    pub read: usize,
    pub error: Option<ReadAtError>,
}

/// 按绝对偏移读取的抽象，供磁盘/内存切片与带缓存读共享。
pub trait ReaderAt: Send + Sync {
    fn read_at(&self, target: &mut [u8], offset: i64) -> ReadAtResult;
}

/// 基于内存切片的 `ReaderAt`，便于单测模拟已落盘数据。
pub struct SliceReaderAt(Vec<u8>);

impl SliceReaderAt {
    /// 用完整字节缓冲构造切片读取器。
    pub fn new(data: Vec<u8>) -> Self {
        Self(data)
    }
}

impl ReaderAt for SliceReaderAt {
    fn read_at(&self, target: &mut [u8], offset: i64) -> ReadAtResult {
        if offset < 0 || offset as usize >= self.0.len() {
            return ReadAtResult {
                read: 0,
                error: Some(ReadAtError::Eof),
            };
        }
        let available = &self.0[offset as usize..];
        let count = available.len().min(target.len());
        target[..count].copy_from_slice(&available[..count]);
        ReadAtResult {
            read: count,
            error: (count < target.len()).then_some(ReadAtError::Eof),
        }
    }
}

/// 底层 `ReaderAt` 之上叠加一块尾部内存缓存，用于 flush 未写完的尾字节。
pub struct ReaderWithCache {
    reader: Box<dyn ReaderAt>,
    /// 缓存对应的数据文件起始偏移。
    cacheOff: i64,
    cache: Vec<u8>,
}

impl ReaderWithCache {
    /// 用底层 reader、尾部 cache 及其起始偏移构造。
    pub fn New(reader: Box<dyn ReaderAt>, cache: Vec<u8>, cacheOff: i64) -> Self {
        Self {
            reader,
            cacheOff,
            cache,
        }
    }

    /// 先读底层，若遇 EOF 且未读满，再从 cache 补齐剩余字节。
    pub fn ReadAt(&self, target: &mut [u8], offset: i64) -> ReadAtResult {
        let mut result = self.reader.read_at(target, offset);
        if result.error != Some(ReadAtError::Eof) || result.read == target.len() {
            return result;
        }
        if result.read > target.len() {
            return ReadAtResult {
                read: result.read,
                error: Some(ReadAtError::Other),
            };
        }

        // 将未 flush 的尾部 cache 接到已读内容之后。
        let cacheStart = (offset + result.read as i64 - self.cacheOff).max(0) as usize;
        if cacheStart >= self.cache.len() {
            return result;
        }
        let remaining = &mut target[result.read..];
        let cacheEnd = (cacheStart + remaining.len()).min(self.cache.len());
        let copied = cacheEnd - cacheStart;
        remaining[..copied].copy_from_slice(&self.cache[cacheStart..cacheEnd]);
        result.read += copied;
        result.error = (result.read < target.len()).then_some(ReadAtError::Eof);
        result
    }
}
