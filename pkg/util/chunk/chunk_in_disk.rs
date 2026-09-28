// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 按 Chunk 粒度把列式数据 spill（溢写）到临时磁盘文件，并支持按索引回读。
//
// `DataInDiskByChunks` 在内存压力大时序列化整块 Chunk；序列化布局含 Chunk 元信息、
// selection、各列的 nullBitmap/data/offsets。磁盘用量由 `diskTracker` 记账。

/// 单字节宽度，用于序列化尺寸计算。
pub const byteLen: i64 = std::mem::size_of::<u8>() as i64;
/// 本机 `isize` 宽度（对应 Go 的 int）。
pub const intLen: i64 = std::mem::size_of::<isize>() as i64;
/// `i64` 宽度。
pub const int64Len: i64 = std::mem::size_of::<i64>() as i64;

/// Chunk 固定头大小：numVirtualRows/capacity/requiredRows/selSize。
pub const chkFixedSize: i64 = intLen * 4;
/// 单列元信息大小：length/nullMapSize/dataSize/offsetSize。
pub const colMetaSize: i64 = int64Len * 4;

// DefaultChunkDataInDiskByChunksPath gives the file name prefix.
/// 临时文件名默认前缀。
pub const DefaultChunkDataInDiskByChunksPath: &str = "defaultChunkDataInDiskByChunksPath";

// DataInDiskByChunks represents some data stored in temporary disk.
// 它只能按 chunk 粒度恢复；字段顺序保持 Go 结构，方便和序列化布局对应。
/// 以 Chunk 为粒度落盘的临时数据容器。
pub struct DataInDiskByChunks {
    pub fieldTypes: Vec<types::FieldType>,
    pub offsetOfEachChunk: Vec<i64>,
    pub totalDataSize: i64,
    pub totalRowNum: i64,
    pub diskTracker: disk::Tracker,
    pub dataFile: diskFileReaderWriter,
    // 读写临时文件前都复用这个缓冲区，Go 代码依赖它承载完整序列化结果。
    pub buf: Vec<u8>,
    pub fileNamePrefixForTest: String,
}

// NewDataInDiskByChunks creates a new DataInDiskByChunks with field types.
/// 按字段类型创建磁盘 Chunk 容器；`fileNamePrefixForTest` 便于测试隔离文件名。
pub fn NewDataInDiskByChunks(
    fieldTypes: Vec<types::FieldType>,
    fileNamePrefixForTest: String,
) -> Box<DataInDiskByChunks> {
    Box::new(DataInDiskByChunks {
        fieldTypes,
        offsetOfEachChunk: Vec::new(),
        totalDataSize: 0,
        totalRowNum: 0,
        // Go 这里 quota 传 -1，表示暂不限制磁盘用量。
        diskTracker: disk::NewTracker(memory::LabelForChunkDataInDiskByChunks, -1),
        dataFile: diskFileReaderWriter::default(),
        buf: Vec::with_capacity(4096),
        fileNamePrefixForTest,
    })
}

impl DataInDiskByChunks {
    /// 初始化临时目录与落盘文件。
    pub fn initDiskFile(&mut self) -> Result<(), errors::Error> {
        disk::CheckAndInitTempDir().map_err(errors::Trace)?;
        let file_name = format!(
            "{}{}{}",
            self.fileNamePrefixForTest,
            DefaultChunkDataInDiskByChunksPath,
            self.diskTracker.Label()
        );
        self.dataFile.initWithFileName(&file_name).map_err(errors::Trace)
    }

    // GetDiskTracker returns the memory tracker of this List.
    /// 返回磁盘用量跟踪器。
    pub fn GetDiskTracker(&self) -> &disk::Tracker {
        &self.diskTracker
    }

    // Add adds a chunk to the DataInDiskByChunks. Caller must make sure field types match.
    // Warning: Do not concurrently call this function.
    /// 序列化并追加一个非空 Chunk 到磁盘；不可并发调用。
    pub fn Add(&mut self, chk: &Chunk) -> Result<(), errors::Error> {
        if let Err(err) = injectChunkInDiskRandomError() {
            return Err(err);
        }
        if chk.NumRows() == 0 {
            return Err(errors::New("Chunk spilled to disk should have at least 1 row"));
        }
        if self.dataFile.file.is_none() {
            self.initDiskFile()?;
        }

        let serializedBytesNum = self.serializeDataToBuf(chk);
        let writeNum = self.dataFile.write(&self.buf)?;
        if writeNum as i64 != serializedBytesNum {
            return Err(errors::New("Some data fail to be spilled to disk"));
        }
        self.offsetOfEachChunk.push(self.totalDataSize);
        self.totalDataSize += serializedBytesNum;
        self.totalRowNum += chk.NumRows() as i64;
        self.dataFile.offWrite += serializedBytesNum;
        self.diskTracker.Consume(serializedBytesNum);
        Ok(())
    }

    // GetTotalBytesInDisk returns total bytes in disk.
    /// 已落盘总字节数。
    pub fn GetTotalBytesInDisk(&self) -> i64 {
        self.totalDataSize
    }

    /// 计算指定下标 Chunk 的序列化字节长度。
    pub fn getChunkSize(&self, chkIdx: usize) -> i64 {
        let totalChunkNum = self.offsetOfEachChunk.len();
        if chkIdx == totalChunkNum - 1 {
            return self.totalDataSize - self.offsetOfEachChunk[chkIdx];
        }
        self.offsetOfEachChunk[chkIdx + 1] - self.offsetOfEachChunk[chkIdx]
    }

    // readFromFisk 保留 Go 中的原函数名拼写；它按 chunk 偏移读取完整序列化字节。
    /// 按偏移把完整序列化字节读入 `buf`（函数名保留 Go 拼写）。
    pub fn readFromFisk(&mut self, chkIdx: usize) -> Result<(), errors::Error> {
        if let Err(err) = injectChunkInDiskRandomError() {
            return Err(err);
        }

        let mut reader = self.dataFile.getSectionReader(self.offsetOfEachChunk[chkIdx]);
        let chkSize = self.getChunkSize(chkIdx);
        if self.buf.capacity() < chkSize as usize {
            self.buf = vec![0; chkSize as usize];
        } else {
            self.buf.resize(chkSize as usize, 0);
        }

        let readByteNum = reader.read_full(&mut self.buf).map_err(errors::Trace)?;
        if readByteNum as i64 != chkSize {
            return Err(errors::New("Fail to restore the spilled chunk"));
        }
        Ok(())
    }

    // GetChunk gets a Chunk from the DataInDiskByChunks by chkIdx.
    /// 从磁盘读回并反序列化为新 Chunk。
    pub fn GetChunk(&mut self, chkIdx: usize) -> Result<Box<Chunk>, errors::Error> {
        self.readFromFisk(chkIdx)?;
        let mut chk = NewEmptyChunk(self.fieldTypes.clone());
        self.deserializeDataToChunk(&mut chk);
        Ok(chk)
    }

    // FillChunk fills a Chunk from the DataInDiskByChunks by chkIdx.
    /// 把磁盘上的 Chunk 反序列化填入已有目标 Chunk。
    pub fn FillChunk(&mut self, srcChkIdx: usize, destChk: &mut Chunk) -> Result<(), errors::Error> {
        self.readFromFisk(srcChkIdx)?;
        self.deserializeDataToChunk(destChk);
        Ok(())
    }

    // Close releases the disk resource.
    /// 关闭并删除临时文件，清零 tracker。
    pub fn Close(&mut self) {
        if let Some(file) = self.dataFile.file.take() {
            // Go 释放 tracker、关闭文件并删除临时文件；错误只通过 terror 记录，不返回给调用方。
            self.diskTracker.Consume(-self.diskTracker.BytesConsumed());
            terror::Call(file.Close());
            terror::Log(os::Remove(file.Name()));
        }
    }

    /// 按本机字节序写入列元信息四元组。
    pub fn serializeColMeta(
        &mut self,
        pos: i64,
        length: i64,
        nullMapSize: i64,
        dataSize: i64,
        offsetSize: i64,
    ) {
        // Go 用 unsafe.Pointer 写入本机字节序 int64；用辅助函数保留同一布局意图。
        put_i64(&mut self.buf, pos, length);
        put_i64(&mut self.buf, pos + int64Len, nullMapSize);
        put_i64(&mut self.buf, pos + int64Len * 2, dataSize);
        put_i64(&mut self.buf, pos + int64Len * 3, offsetSize);
    }

    /// 序列化变长列 offsets 数组。
    pub fn serializeOffset(&mut self, pos: &mut i64, offsets: &[i64], offsetSize: i64) {
        self.buf.resize((*pos + offsetSize) as usize, 0);
        for offset in offsets {
            put_i64(&mut self.buf, *pos, *offset);
            *pos += int64Len;
        }
    }

    /// 序列化 Chunk 固定头与 selection vector。
    pub fn serializeChunkData(&mut self, pos: &mut i64, chk: &Chunk, selSize: i64) {
        self.buf.resize(chkFixedSize as usize, 0);
        put_int(&mut self.buf, *pos, chk.numVirtualRows as isize);
        put_int(&mut self.buf, *pos + intLen, chk.capacity as isize);
        put_int(&mut self.buf, *pos + intLen * 2, chk.requiredRows as isize);
        put_int(&mut self.buf, *pos + intLen * 3, selSize as isize);
        *pos += chkFixedSize;

        self.buf.resize((*pos + selSize) as usize, 0);
        if let Some(sel) = &chk.sel {
            for idx in sel {
                put_int(&mut self.buf, *pos, *idx as isize);
                *pos += intLen;
            }
        }
    }

    /// 逐列写入元信息、nullBitmap、data 与 offsets。
    pub fn serializeColumns(&mut self, pos: &mut i64, chk: &Chunk) {
        for col in &chk.columns {
            self.buf.resize((*pos + colMetaSize) as usize, 0);
            let nullMapSize = col.nullBitmap.len() as i64 * byteLen;
            let dataSize = col.data.len() as i64 * byteLen;
            let offsetSize = col.offsets.len() as i64 * int64Len;
            self.serializeColMeta(*pos, col.length as i64, nullMapSize, dataSize, offsetSize);
            *pos += colMetaSize;

            self.buf.extend_from_slice(&col.nullBitmap);
            self.buf.extend_from_slice(&col.data);
            *pos += nullMapSize + dataSize;
            self.serializeOffset(pos, &col.offsets, offsetSize);
        }
    }

    // Serialized format of a chunk:
    // chunk data: | numVirtualRows | capacity | requiredRows | selSize | sel... |
    // column data: | length | nullMapSize | dataSize | offsetSize | nullBitmap... | data... | offsets... |
    /// 将整个 Chunk 序列化到内部 `buf`，返回字节数。
    pub fn serializeDataToBuf(&mut self, chk: &Chunk) -> i64 {
        let selSize = chk.sel.as_ref().map(|s| s.len()).unwrap_or(0) as i64 * intLen;
        let mut totalBytes = chkFixedSize + selSize;
        for col in &chk.columns {
            let nullMapSize = col.nullBitmap.len() as i64 * byteLen;
            let dataSize = col.data.len() as i64 * byteLen;
            let offsetSize = col.offsets.len() as i64 * int64Len;
            totalBytes += colMetaSize + nullMapSize + dataSize + offsetSize;
        }

        if self.buf.capacity() < totalBytes as usize {
            self.buf = Vec::with_capacity(totalBytes as usize);
        }
        self.buf.clear();

        let mut pos = 0;
        self.serializeChunkData(&mut pos, chk, selSize);
        self.serializeColumns(&mut pos, chk);
        totalBytes
    }

    /// 读取列元信息四元组。
    pub fn deserializeColMeta(&self, pos: &mut i64) -> (i64, i64, i64, i64) {
        let length = get_i64(&self.buf, *pos);
        *pos += int64Len;
        let nullMapSize = get_i64(&self.buf, *pos);
        *pos += int64Len;
        let dataSize = get_i64(&self.buf, *pos);
        *pos += int64Len;
        let offsetSize = get_i64(&self.buf, *pos);
        *pos += int64Len;
        (length, nullMapSize, dataSize, offsetSize)
    }

    /// 反序列化 selection vector。
    pub fn deserializeSel(&self, chk: &mut Chunk, pos: &mut i64, selSize: usize) {
        let selLen = selSize as i64 / intLen;
        let mut sel = vec![0usize; selLen as usize];
        for i in 0..selLen as usize {
            sel[i] = get_int(&self.buf, *pos) as usize;
            *pos += intLen;
        }
        chk.sel = Some(sel);
    }

    /// 反序列化 Chunk 头字段与可选 `sel`。
    pub fn deserializeChunkData(&self, chk: &mut Chunk, pos: &mut i64) {
        chk.numVirtualRows = get_int(&self.buf, *pos) as usize;
        *pos += intLen;
        chk.capacity = get_int(&self.buf, *pos) as usize;
        *pos += intLen;
        chk.requiredRows = get_int(&self.buf, *pos) as usize;
        *pos += intLen;
        let selSize = get_int(&self.buf, *pos) as usize;
        *pos += intLen;
        if selSize != 0 {
            self.deserializeSel(chk, pos, selSize);
        }
    }

    /// 反序列化 offsets 到目标切片。
    pub fn deserializeOffsets(&self, dst: &mut [i64], pos: &mut i64) {
        for item in dst {
            *item = get_i64(&self.buf, *pos);
            *pos += int64Len;
        }
    }

    /// 反序列化各列 payload 到 `chk.columns`。
    pub fn deserializeColumns(&self, chk: &mut Chunk, pos: &mut i64) {
        for col in &mut chk.columns {
            let (length, nullMapSize, dataSize, offsetSize) = self.deserializeColMeta(pos);

            col.nullBitmap.resize(nullMapSize as usize, 0);
            col.data.resize(dataSize as usize, 0);
            col.offsets.resize((offsetSize / int64Len) as usize, 0);

            col.length = length as usize;
            col.nullBitmap
                .copy_from_slice(&self.buf[*pos as usize..(*pos + nullMapSize) as usize]);
            *pos += nullMapSize;
            col.data
                .copy_from_slice(&self.buf[*pos as usize..(*pos + dataSize) as usize]);
            *pos += dataSize;
            self.deserializeOffsets(&mut col.offsets, pos);
        }
    }

    /// 从 `buf` 完整反序列化到目标 Chunk。
    pub fn deserializeDataToChunk(&self, chk: &mut Chunk) {
        let mut pos = 0;
        self.deserializeChunkData(chk, &mut pos);
        self.deserializeColumns(chk, &mut pos);
    }

    // NumRows returns total spilled row number.
    /// 已 spill 的总行数。
    pub fn NumRows(&self) -> i64 {
        self.totalRowNum
    }

    // NumChunks returns total spilled chunk number.
    /// 已 spill 的 Chunk 个数。
    pub fn NumChunks(&self) -> usize {
        self.offsetOfEachChunk.len()
    }
}

// injectChunkInDiskRandomError 对应 Go failpoint：测试中随机返回错误或短暂 sleep。
/// failpoint：测试中随机注入错误或短暂延迟。
pub fn injectChunkInDiskRandomError() -> Result<(), errors::Error> {
    let mut err: Option<errors::Error> = None;
    failpoint::Inject("ChunkInDiskError", |val: failpoint::Value| {
        if val.as_bool() {
            let randNum = rand::Int31n(10000);
            if randNum < 3 {
                err = Some(errors::New("random error is triggered"));
            } else if randNum < 6 {
                let delayTime = rand::Int31n(10) + 5;
                time::Sleep(std::time::Duration::from_millis(delayTime as u64));
            }
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

// 以下 helper 只为表达 Go unsafe 按本机字节序写入 int/int64 的意图。
/// 保证缓冲区足以容纳从 `pos` 起 `width` 字节。
fn ensure_len(buf: &mut Vec<u8>, pos: i64, width: usize) {
    let need = pos as usize + width;
    if buf.len() < need {
        buf.resize(need, 0);
    }
}

/// 按本机字节序写入 `i64`。
fn put_i64(buf: &mut Vec<u8>, pos: i64, val: i64) {
    ensure_len(buf, pos, 8);
    buf[pos as usize..pos as usize + 8].copy_from_slice(&val.to_ne_bytes());
}

/// 按本机字节序读取 `i64`。
fn get_i64(buf: &[u8], pos: i64) -> i64 {
    i64::from_ne_bytes(buf[pos as usize..pos as usize + 8].try_into().unwrap())
}

/// 按本机字节序写入 `isize`（对应 Go int）。
fn put_int(buf: &mut Vec<u8>, pos: i64, val: isize) {
    let bytes = val.to_ne_bytes();
    ensure_len(buf, pos, bytes.len());
    buf[pos as usize..pos as usize + bytes.len()].copy_from_slice(&bytes);
}

/// 按本机字节序读取 `isize`。
fn get_int(buf: &[u8], pos: i64) -> isize {
    let mut bytes = [0u8; std::mem::size_of::<isize>()];
    let width = bytes.len();
    bytes.copy_from_slice(&buf[pos as usize..pos as usize + width]);
    isize::from_ne_bytes(bytes)
}
