// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 对象存储 ReaderAt/Seek/Close 包装，以及小 row group 内存预读。
//
// 通过 skip buffer 减少小跨度前跳时的 Seek（对象存储 Seek 可能意味着重新 Open）。

// 对象存储 ReaderAt/Seek/Close 包装，以及小 row group 的内存预读策略。
//
// Copied from https://github.com/apache/arrow-go/blob/bbf7ab7523a6411e25c7a08566a40e8759cc6c13/parquet/file/row_group_reader.go#L32C1-L34C2
// pub const maxDictHeaderSize: i64 = 100;
//
// rowGroupInMemoryThreshold controls when we preload an entire row group.
// If the row-group size is no larger than this threshold, we read it once
// into memory and let all column readers share that buffer. This reduces
// number of GET requests for files with many small columns, where first-byte
// latency can dominate read time. 128 MiB is an heuristic value which we can
// tolerate the extra memory usage for row group.
// pub static mut rowGroupInMemoryThreshold: i64 = 128 * units::MiB;
//
// readerAtSeekerCloser 对应 Go 组合接口：Parquet reader 需要 ReaderAt、Seeker 和 Closer。
// pub trait readerAtSeekerCloser: io::ReaderAt + io::Seeker + io::Closer {}
//
// readerWrapper implements parquet.ReaderAtSeeker.
// readerWrapper 包装 storeapi.ReadSeekCloser，并用 skipBuf 优化小跨度前跳。
// pub struct readerWrapper {
//     pub ReadSeekCloser: storeapi::ReadSeekCloser,
//     pub lastOff: i64,
//     pub skipBuf: Vec<u8>,
// }
//
// impl readerWrapper {
//     pub fn readNBytes(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
//         let (n, err) = io::ReadFull(self, buf);
//         if err.is_some() && err != Some(io::EOF) {
//             return Err(errors::Trace(err.unwrap()));
//         }
//         if n != buf.len() {
//             return Err(Error::new(format!("error reading {} bytes, only read {} bytes", buf.len(), n)));
//         }
//         Ok(n)
//     }
//
// ReadAt implement ReaderAt interface
//     pub fn ReadAt(&mut self, buf: &mut [u8], off: i64) -> Result<usize, Error> {
// Go 这里尽量减少 Seek 调用；对象存储底层 Seek 可能意味着重新打开文件。
//         let gap = (off - self.lastOff) as i32;
//         if gap < 0 || gap as usize > self.skipBuf.capacity() {
//             self.Seek(off, io::SeekStart)?;
//         } else {
//             self.skipBuf.resize(gap as usize, 0);
//             if let Err(err) = self.readNBytes(&mut self.skipBuf) {
//                 return Err(err);
//             }
//         }
//
//         let read = self.readNBytes(buf)?;
//         self.lastOff = off + read as i64;
//         Ok(buf.len())
//     }
//
// Seek implement Seeker interface
//     pub fn Seek(&mut self, offset: i64, whence: i32) -> Result<i64, Error> {
//         let newOffset = self.ReadSeekCloser.Seek(offset, whence)?;
//         self.lastOff = newOffset;
//         Ok(newOffset)
//     }
//
//     pub fn Write(&mut self, _p: &[u8]) -> Result<usize, Error> {
//         Err(errors::New("unsupported operation"))
//     }
// }
//
// newReaderWrapper 对应 Go 的对象存储打开逻辑。
// failpoint 允许测试把本地 reader 换成感知 context 的包装器；保留调用点。
// pub fn newReaderWrapper(
//     ctx: context::Context,
//     store: storeapi::Storage,
//     path: String,
//     opts: Option<storeapi::ReaderOption>,
// ) -> Result<Box<readerWrapper>, Error> {
//     let mut reader = store.Open(ctx.clone(), path, opts.clone()).map_err(errors::Trace)?;
//     failpoint::InjectCall("interceptParquetReader", &mut reader, ctx);
//
//     let mut lastOff = 0;
//     if let Some(opts) = opts {
//         if let Some(startOffset) = opts.StartOffset {
//             lastOff = startOffset;
//         }
//     }
//
//     Ok(Box::new(readerWrapper {
//         ReadSeekCloser: reader,
//         lastOff,
//         skipBuf: vec![0u8; defaultBufSize],
//     }))
// }
//
// rowGroupRange 对应 Go 的范围聚合结构，记录整个 row group 和各列 chunk 的起止 offset。
// #[derive(Clone)]
// pub struct rowGroupRange {
//     pub start: i64,
//     pub end: i64,
//     pub columnStarts: Vec<i64>,
//     pub columnEnds: Vec<i64>,
// }
//
// impl rowGroupRange {
//     pub fn add(&mut self, start: i64, end: i64) {
//         self.start = std::cmp::min(self.start, start);
//         self.end = std::cmp::max(self.end, end);
//         self.columnStarts.push(start);
//         self.columnEnds.push(end);
//     }
// }
//
// inMemoryReaderBase reads one row group into memory and serves ReaderAt.
// inMemoryReaderBase 对应 Go 的共享内存底座，一个 row group 只预读一次。
// pub struct inMemoryReaderBase {
//     pub buffer: Vec<u8>,
//     pub rowGroup: rowGroupRange,
// }
//
// pub fn newInMemoryReaderBase(
//     ctx: context::Context,
//     store: storeapi::Storage,
//     path: String,
//     rowGroup: rowGroupRange,
// ) -> Result<inMemoryReaderBase, Error> {
//     let mut base = inMemoryReaderBase {
//         buffer: vec![0u8; (rowGroup.end - rowGroup.start) as usize],
//         rowGroup,
//     };
//     base.loadRowGroup(ctx, store, path)?;
//     Ok(base)
// }
//
// impl inMemoryReaderBase {
//     pub fn ReadAt(&self, p: &mut [u8], off: i64) -> Result<usize, Error> {
//         let start = off - self.rowGroup.start;
//         let groupSize = self.rowGroup.end - self.rowGroup.start;
//
// Go 的 sanity check：正常情况下 ColumnChunkReader 不应读取 row group 起点之前的数据。
//         if start < 0 {
//             return Err(errors::Errorf(format!(
//                 "invalid offset {} before current row group start {}",
//                 off, self.rowGroup.start
//             )));
//         }
//         if start >= groupSize {
//             return Err(io::EOF);
//         }
//
//         let n = copy_slice(p, &self.buffer[start as usize..groupSize as usize]);
//         if n < p.len() {
//             return Err(io::EOF);
//         }
//         Ok(n)
//     }
//
//     pub fn loadRowGroup(
//         &mut self,
//         ctx: context::Context,
//         store: storeapi::Storage,
//         path: String,
//     ) -> Result<(), Error> {
//         let rg = self.rowGroup.clone();
//         let (mut eg, egCtx) = util::NewErrorGroupWithRecoverWithCtx(ctx);
//         eg.SetLimit(8);
//         let mut readStart = rg.start;
//         while readStart < rg.end {
//             let batchSize = std::cmp::min(simplesst::ConcurrentReaderBufferSizePerConc as i64, rg.end - readStart);
//             let start = readStart;
//             readStart += batchSize;
//             let offset = start - rg.start;
// Go 并发按块读取对象存储范围；保留 error group 与 offset 切片写入语义。
//             eg.Go(|| {
//                 objstore::ReadDataInRange(
//                     egCtx.clone(),
//                     store.clone(),
//                     path.clone(),
//                     start,
//                     &mut self.buffer[offset as usize..(offset + batchSize) as usize],
//                 )?;
//                 Ok(())
//             });
//         }
//         eg.Wait()
//     }
// }
//
// inMemoryReaderWrapper 对应 Go 的 per-column wrapper：共享 base，但每列维护独立 seek pos。
// pub struct inMemoryReaderWrapper {
//     pub base: inMemoryReaderBase,
//     pub fileSize: i64,
//     pub pos: i64,
// }
//
// impl inMemoryReaderWrapper {
//     pub fn ReadAt(&self, p: &mut [u8], off: i64) -> Result<usize, Error> {
//         self.base.ReadAt(p, off)
//     }
//
//     pub fn Seek(&mut self, offset: i64, whence: i32) -> Result<i64, Error> {
//         let base = match whence {
//             io::SeekStart => 0,
//             io::SeekCurrent => self.pos,
//             io::SeekEnd => self.fileSize,
//             _ => return Err(errors::Errorf(format!("invalid whence {}", whence))),
//         };
//         let newPos = base + offset;
//         if newPos < 0 {
//             return Err(errors::Errorf(format!("invalid offset {}", newPos)));
//         }
//         self.pos = newPos;
//         Ok(newPos)
//     }
//
//     pub fn Close(&mut self) -> Result<(), Error> {
//         Ok(())
//     }
// }
//
// Copied from https://github.com/apache/arrow-go/blob/bbf7ab7523a6411e25c7a08566a40e8759cc6c13/parquet/file/row_group_reader.go
// rowGroupRangeFromMeta 对应 Go 的 footer metadata 范围计算，包含 PARQUET-816 旧文件 padding 兼容。
// pub fn rowGroupRangeFromMeta(
//     fileMeta: &metadata::FileMetaData,
//     idx: i32,
// ) -> Result<rowGroupRange, Error> {
//     let rg = fileMeta.RowGroup(idx);
//     let mut ranges = rowGroupRange {
//         start: math::MaxInt64,
//         end: 0,
//         columnStarts: Vec::new(),
//         columnEnds: Vec::new(),
//     };
//
//     for i in 0..rg.NumColumns() {
//         let col = rg.ColumnChunk(i)
//             .map_err(|err| Error::new(format!("cannot get column chunk {} metadata: {}", i, err)))?;
//         let mut colStart = col.DataPageOffset();
//         if col.HasDictionaryPage() && col.DictionaryPageOffset() > 0 {
//             colStart = std::cmp::min(colStart, col.DictionaryPageOffset());
//         }
//
//         let mut colLen = col.TotalCompressedSize();
//         if fileMeta.WriterVersion().LessThan(metadata::Parquet816FixedVersion) {
//             let sourceSz = fileMeta.GetSourceFileSize();
// Parquet MR 1.2.8 及之前可能没有把 dictionary page header 算进 compressed size。
//             if colStart < 0 || colLen < 0 {
//                 return Err(errors::Errorf(format!(
//                     "invalid column chunk metadata, offset ({}) and length ({}) should both be positive",
//                     colStart, colLen
//                 )));
//             }
//             if colStart > sourceSz || colLen > sourceSz {
//                 return Err(errors::Errorf(format!(
//                     "invalid column chunk metadata, offset ({}) and length ({}) must both be less than total source size ({})",
//                     colStart, colLen, sourceSz
//                 )));
//             }
//             let bytesRemain = sourceSz - (colStart + colLen);
//             let padding = std::cmp::min(maxDictHeaderSize, bytesRemain);
//             colLen += padding;
//         }
//
//         ranges.add(colStart, colStart + colLen);
//     }
//     Ok(ranges)
// }
// */
use crate::{Error, Result};
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
/// ReadAt 前跳时可吞掉的最大 gap（skip buffer 容量）。
pub const DEFAULT_BUFFER_SIZE: usize = 64 * 1024;
/// 旧 parquet-mr 字典页头额外预留长度。
pub const MAX_DICT_HEADER_SIZE: i64 = 100;
/// 整组预读阈值：≤128MiB 的 row group 一次读入内存共享给各列 reader。
pub const ROW_GROUP_IN_MEMORY_THRESHOLD: i64 = 128 * 1024 * 1024;
#[derive(Clone, Debug)]
/// 内存中的 ReaderAtSeeker 实现，模拟对象存储打开后的随机读。
pub struct ReaderWrapper {
    data: Arc<Vec<u8>>,
    position: u64,
    last_offset: u64,
    skip_capacity: usize,
    closed: bool,
}
impl ReaderWrapper {
    /// 从字节缓冲构造，可选起始偏移。
    pub fn new(data: Arc<Vec<u8>>, start: u64) -> Result<Self> {
        if start > data.len() as u64 {
            return Err(Error("start offset beyond file".into()));
        }
        Ok(Self {
            data,
            position: start,
            last_offset: start,
            skip_capacity: DEFAULT_BUFFER_SIZE,
            closed: false,
        })
    }
    /// 实现 ReaderAt：gap 在 skip 容量内则推进 position，否则 Seek。
    pub fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize> {
        if self.closed {
            return Err(Error("reader is closed".into()));
        }
        // 负 gap 或超过 skip_capacity 时必须 Seek，避免错误复用 last_offset。
        let gap = offset as i128 - self.last_offset as i128;
        if gap < 0 || gap as usize > self.skip_capacity {
            self.seek(SeekFrom::Start(offset))?;
        } else {
            self.position = self
                .position
                .checked_add(gap as u64)
                .ok_or_else(|| Error("offset overflow".into()))?;
        }
        let read = self.read(buf)?;
        if read != buf.len() {
            return Err(Error(format!(
                "error reading {} bytes, only read {read} bytes",
                buf.len()
            )));
        }
        self.last_offset = offset + read as u64;
        Ok(read)
    }
    /// 标记关闭；后续 read_at 失败。
    pub fn close(&mut self) {
        self.closed = true;
    }
}
/// 顺序读：从当前 position 拷贝到 buf。
impl Read for ReaderWrapper {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let start = self.position as usize;
        if start >= self.data.len() {
            return Ok(0);
        }
        let count = buf.len().min(self.data.len() - start);
        buf[..count].copy_from_slice(&self.data[start..start + count]);
        self.position += count as u64;
        Ok(count)
    }
}
/// 更新 position 与 last_offset。
impl Seek for ReaderWrapper {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        let next = match from {
            SeekFrom::Start(v) => v as i128,
            SeekFrom::Current(v) => self.position as i128 + v as i128,
            SeekFrom::End(v) => self.data.len() as i128 + v as i128,
        };
        if next < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "negative seek",
            ));
        }
        self.position = next as u64;
        self.last_offset = self.position;
        Ok(self.position)
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一个 row group 在文件中的字节区间及各列起止。
pub struct RowGroupRange {
    pub start: i64,
    pub end: i64,
    pub column_starts: Vec<i64>,
    pub column_ends: Vec<i64>,
}
impl RowGroupRange {
    /// 并入一列区间，扩展整体 [start, end)。
    pub fn add(&mut self, start: i64, end: i64) {
        self.start = self.start.min(start);
        self.end = self.end.max(end);
        self.column_starts.push(start);
        self.column_ends.push(end);
    }
}
#[derive(Clone, Debug)]
/// 把整个 row group 切片缓存在内存中的 ReaderAt 基类。
pub struct InMemoryReaderBase {
    buffer: Arc<Vec<u8>>,
    pub row_group: RowGroupRange,
}
impl InMemoryReaderBase {
    /// 拷贝 [range.start, range.end) 到内部缓冲。
    pub fn new(file: &[u8], range: RowGroupRange) -> Result<Self> {
        if range.start < 0 || range.end < range.start || range.end as usize > file.len() {
            return Err(Error("invalid row group range".into()));
        }
        Ok(Self {
            buffer: Arc::new(file[range.start as usize..range.end as usize].to_vec()),
            row_group: range,
        })
    }
    /// 相对文件绝对 offset 读取；offset 必须 ≥ row_group.start。
    pub fn read_at(&self, out: &mut [u8], offset: i64) -> Result<usize> {
        let start = offset - self.row_group.start;
        if start < 0 {
            return Err(Error(format!(
                "invalid offset {offset} before current row group start {}",
                self.row_group.start
            )));
        }
        if start as usize >= self.buffer.len() {
            return Err(Error("EOF".into()));
        }
        let count = out.len().min(self.buffer.len() - start as usize);
        out[..count].copy_from_slice(&self.buffer[start as usize..start as usize + count]);
        if count < out.len() {
            return Err(Error("EOF".into()));
        }
        Ok(count)
    }
}
#[derive(Clone, Debug)]
/// 列块元数据：数据页/字典页偏移与压缩大小。
pub struct ColumnChunkMeta {
    pub data_page_offset: i64,
    pub dictionary_page_offset: Option<i64>,
    pub total_compressed_size: i64,
}
#[derive(Clone, Debug)]
/// 文件级元数据：源大小、是否旧 parquet-mr、各 row group 列块。
pub struct FileMeta {
    pub source_size: i64,
    pub old_parquet_mr: bool,
    pub row_groups: Vec<Vec<ColumnChunkMeta>>,
}
/// 由 FileMeta 计算第 index 个 row group 的字节范围。
/// 旧 parquet-mr 会额外加上字典页头余量。
pub fn row_group_range_from_meta(meta: &FileMeta, index: usize) -> Result<RowGroupRange> {
    let group = meta
        .row_groups
        .get(index)
        .ok_or_else(|| Error(format!("row group index {index} out of range")))?;
    let mut range = RowGroupRange {
        start: i64::MAX,
        ..RowGroupRange::default()
    };
    for column in group {
        let start = column
            .dictionary_page_offset
            .filter(|v| *v > 0)
            .map_or(column.data_page_offset, |v| v.min(column.data_page_offset));
        let mut length = column.total_compressed_size;
        // 旧写入器可能把字典页头算在 compressed size 之外，需补 MAX_DICT_HEADER_SIZE。
        if meta.old_parquet_mr {
            if start < 0 || length < 0 || start > meta.source_size || length > meta.source_size {
                return Err(Error(format!(
                    "invalid column chunk metadata, offset ({start}) and length ({length})"
                )));
            }
            let bytes_remaining = meta.source_size.wrapping_sub(start.wrapping_add(length));
            length = length.wrapping_add(MAX_DICT_HEADER_SIZE.min(bytes_remaining));
        }
        range.add(start, start.wrapping_add(length));
    }
    Ok(range)
}
/// Go 风格别名。
pub fn newReaderWrapper(data: Arc<Vec<u8>>, start: u64) -> Result<ReaderWrapper> {
    ReaderWrapper::new(data, start)
}
/// Go 风格别名。
pub fn rowGroupRangeFromMeta(m: &FileMeta, i: usize) -> Result<RowGroupRange> {
    row_group_range_from_meta(m, i)
}
