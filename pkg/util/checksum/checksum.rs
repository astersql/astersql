// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 带 CRC-32 块头的校验和读写封装。
//
// 对应 Go `util/checksum`。每个编码块固定 `checksumBlockSize` 字节：
// 前 4 字节为载荷的 IEEE CRC-32（小端），其后为用户数据。
// Writer 缓冲并刷盘；Reader 按用户偏移映射到编码块并校验。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::Mutex;

/// 对齐 Go `error` 的字符串错误类型。
pub type GoError = String;

/// 对齐 Go `io.WriteCloser` 的可写可关闭草稿 trait。
pub trait WriteCloserDraft {
    /// 写入字节，返回已写长度与可选错误。
    fn Write(&mut self, p: &[u8]) -> (usize, Option<GoError>);
    /// 关闭底层资源。
    fn Close(&mut self) -> Option<GoError>;
}

impl<T: WriteCloserDraft + ?Sized> WriteCloserDraft for &mut T {
    fn Write(&mut self, p: &[u8]) -> (usize, Option<GoError>) {
        (**self).Write(p)
    }

    fn Close(&mut self) -> Option<GoError> {
        (**self).Close()
    }
}

/// 对齐 Go `io.ReaderAt`：按偏移读取，不移动游标。
pub trait ReaderAtDraft {
    /// 从 `off` 起填充 `p`，返回读到的字节数与可选错误。
    fn ReadAt(&self, p: &mut [u8], off: i64) -> (usize, Option<GoError>);
}

impl<T: ReaderAtDraft + ?Sized> ReaderAtDraft for &T {
    fn ReadAt(&self, p: &mut [u8], off: i64) -> (usize, Option<GoError>) {
        (**self).ReadAt(p, off)
    }
}

// The size of one encoded checksum block.
/// 单个编码块总长度（含 CRC 头）。
pub const checksumBlockSize: usize = 1024;
// CRC-32 is stored as a four-byte little-endian value.
/// CRC-32 校验头字节数（小端 u32）。
pub const checksumSize: usize = 4;
// The user payload available in one encoded block.
/// 单块内可用的用户载荷字节数。
pub const checksumPayloadSize: usize = checksumBlockSize - checksumSize;

/// Reader 读块缓冲池，避免频繁分配 1024 字节临时缓冲。
static checksumReaderBufPool: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// 从缓冲池租借的读块缓冲；Drop 时归还。
struct ReaderBuffer(Option<Vec<u8>>);

impl ReaderBuffer {
    /// 从池中取出一块，池空则新建。
    fn acquire() -> Self {
        let buffer = checksumReaderBufPool
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop()
            .unwrap_or_else(|| vec![0; checksumBlockSize]);
        Self(Some(buffer))
    }

    fn bytes(&mut self) -> &mut [u8] {
        self.0.as_mut().unwrap()
    }
}

impl Drop for ReaderBuffer {
    fn drop(&mut self) {
        // 归还缓冲供后续 ReadAt 复用。
        checksumReaderBufPool
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(self.0.take().unwrap());
    }
}

/// Adds an IEEE CRC-32 header to every block written to the underlying object.
/// 向底层写入时，为每个块附加 IEEE CRC-32 头。
pub struct Writer<W: WriteCloserDraft> {
    /// 已锁存的持久错误；一旦设置，后续写操作直接返回。
    err: Option<GoError>,
    w: W,
    /// 编码后待写出的整块缓冲（CRC + 载荷）。
    buf: Vec<u8>,
    /// 当前未刷盘的用户载荷。
    payload: Vec<u8>,
    /// `payload` 中已使用字节数。
    payloadUsed: usize,
    /// 已成功刷盘的用户字节累计，供缓存偏移查询。
    flushedUserDataCnt: i64,
}

/// 包装底层 `WriteCloser`，构造带校验头的 Writer。
pub fn NewWriter<W: WriteCloserDraft>(w: W) -> Writer<W> {
    Writer {
        err: None,
        w,
        buf: vec![0; checksumBlockSize],
        payload: vec![0; checksumPayloadSize],
        payloadUsed: 0,
        flushedUserDataCnt: 0,
    }
}

impl<W: WriteCloserDraft> Writer<W> {
    /// 当前载荷缓冲剩余可写字节数。
    pub fn AvailableSize(&self) -> usize {
        checksumPayloadSize - self.payloadUsed
    }

    /// 写入用户数据；缓冲满时自动 Flush 成带 CRC 的编码块。
    pub fn Write(&mut self, mut p: &[u8]) -> (usize, Option<GoError>) {
        let mut n = 0;
        // 剩余空间不够时先填满并刷盘，再继续写后续数据。
        while p.len() > self.AvailableSize() && self.err.is_none() {
            let copied = copy_to(&mut self.payload[self.payloadUsed..], p);
            self.payloadUsed += copied;
            if let Some(err) = self.Flush() {
                return (n, Some(err));
            }
            n += copied;
            p = &p[copied..];
        }

        if let Some(err) = &self.err {
            return (n, Some(err.clone()));
        }

        let copied = copy_to(&mut self.payload[self.payloadUsed..], p);
        self.payloadUsed += copied;
        n += copied;
        (n, None)
    }

    /// 尚未刷盘的用户字节数。
    pub fn Buffered(&self) -> usize {
        self.payloadUsed
    }

    /// 将当前载荷编码为 CRC 块并写出；空缓冲时为 no-op。
    pub fn Flush(&mut self) -> Option<GoError> {
        if let Some(err) = &self.err {
            return Some(err.clone());
        }
        if self.payloadUsed == 0 {
            return None;
        }

        // 计算载荷 CRC，拼成「4 字节校验 + 载荷」后写入底层。
        let checksum = crc32fast::hash(&self.payload[..self.payloadUsed]);
        self.buf[..checksumSize].copy_from_slice(&checksum.to_le_bytes());
        self.buf[checksumSize..checksumSize + self.payloadUsed]
            .copy_from_slice(&self.payload[..self.payloadUsed]);

        let write_len = checksumSize + self.payloadUsed;
        let (written, mut err) = self.w.Write(&self.buf[..write_len]);
        // Keep the Go implementation's threshold: only user bytes count here.
        // 对齐 Go：仅用用户字节数判断 short write，不含 CRC 头。
        if written < self.payloadUsed && err.is_none() {
            err = Some("short write".to_owned());
        }
        if let Some(err) = err {
            self.err = Some(err.clone());
            return Some(err);
        }

        self.flushedUserDataCnt += self.payloadUsed as i64;
        self.payloadUsed = 0;
        None
    }

    /// 返回尚未刷盘的载荷切片。
    pub fn GetCache(&self) -> &[u8] {
        &self.payload[..self.payloadUsed]
    }

    /// 已刷盘用户数据的逻辑偏移（不含 CRC 头）。
    pub fn GetCacheDataOffset(&self) -> i64 {
        self.flushedUserDataCnt
    }

    /// 先 Flush 再关闭底层 Writer。
    pub fn Close(&mut self) -> Option<GoError> {
        if let Some(err) = self.Flush() {
            return Some(err);
        }
        self.w.Close()
    }
}

impl<W: WriteCloserDraft> WriteCloserDraft for Writer<W> {
    fn Write(&mut self, p: &[u8]) -> (usize, Option<GoError>) {
        Writer::Write(self, p)
    }

    fn Close(&mut self) -> Option<GoError> {
        Writer::Close(self)
    }
}

/// Reads payload bytes after validating every encoded checksum block.
/// 读取时先校验每个编码块的 CRC，再返回用户载荷。
pub struct Reader<R: ReaderAtDraft> {
    r: R,
}

/// 包装底层 `ReaderAt`，构造带校验的 Reader。
pub fn NewReader<R: ReaderAtDraft>(r: R) -> Reader<R> {
    Reader { r }
}

/// CRC 不匹配或块过短时返回的错误文案，对齐 Go。
pub const errChecksumFail: &str = "error checksum";

impl<R: ReaderAtDraft> Reader<R> {
    /// 按用户逻辑偏移读取；内部映射到编码块偏移并逐块验 CRC。
    pub fn ReadAt(&self, mut p: &mut [u8], off: i64) -> (usize, Option<GoError>) {
        if p.is_empty() {
            return (0, None);
        }

        // 用户偏移 → 块内载荷偏移 + 编码文件游标。
        let mut offsetInPayload = (off % checksumPayloadSize as i64) as usize;
        let mut cursor = off / checksumPayloadSize as i64 * checksumBlockSize as i64;
        let mut buffer = ReaderBuffer::acquire();
        let mut nn = 0;

        while !p.is_empty() {
            let (n, read_err) = self.r.ReadAt(buffer.bytes(), cursor);
            if let Some(read_err) = read_err {
                if n == 0 || read_err != "EOF" {
                    return (nn, Some(read_err));
                }
                // A partial read accompanied by EOF still contains a final block to verify.
                // 带 EOF 的部分读仍可能是最后一块，需继续校验。
            }
            if n < checksumSize {
                return (nn, Some(errChecksumFail.to_owned()));
            }

            cursor += n as i64;
            let bytes = buffer.bytes();
            // 比对块头 CRC 与载荷实际哈希。
            let expected = u32::from_le_bytes(bytes[..checksumSize].try_into().unwrap());
            let actual = crc32fast::hash(&bytes[checksumSize..n]);
            if expected != actual {
                return (nn, Some(errChecksumFail.to_owned()));
            }

            let start = checksumSize + offsetInPayload;
            let copied = copy_to(p, &bytes[start..n]);
            nn += copied;
            p = p.split_at_mut(copied).1;
            offsetInPayload = 0;
        }

        (nn, None)
    }
}

impl<R: ReaderAtDraft> ReaderAtDraft for Reader<R> {
    fn ReadAt(&self, p: &mut [u8], off: i64) -> (usize, Option<GoError>) {
        Reader::ReadAt(self, p, off)
    }
}

/// 将 `src` 尽可能拷入 `dst`，返回实际拷贝字节数。
fn copy_to(dst: &mut [u8], src: &[u8]) -> usize {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
}
