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

// AES-CTR 流式加解密层：按固定块大小缓冲写入，并支持按偏移随机读解密。
//
// 对应 Go `pkg/util/encrypt/aes_layer.go`，常与 checksum 管线组合，用于备份/导入导出等
// 需要边写边加密、边读边解密的场景。CTR 的 IV 由随机 nonce 与块计数器拼接而成。

use aes::Aes128;
use cipher::{KeyIvInit, StreamCipher};
use rand::RngCore;
use std::io;

/// 对齐 Go `error` 的字符串错误类型。
pub type GoError = String;

/// 非法加密块大小时的错误消息。
pub const errInvalidBlockSize: &str = "invalid encrypt block size";
/// 默认加密缓冲块大小（字节），须为 16 的倍数。
pub const defaultEncryptBlockSize: i64 = 1024;

/// 可关闭的写入端抽象（对应 Go `io.WriteCloser`）。
pub trait WriteCloser {
    fn write(&mut self, p: &[u8]) -> (usize, Option<GoError>);
    fn close(&mut self) -> Option<GoError>;
}

impl<T: WriteCloser + ?Sized> WriteCloser for &mut T {
    fn write(&mut self, p: &[u8]) -> (usize, Option<GoError>) {
        (**self).write(p)
    }

    fn close(&mut self) -> Option<GoError> {
        (**self).close()
    }
}

/// 按偏移读取的抽象（对应 Go `io.ReaderAt`）。
pub trait ReaderAt {
    fn read_at(&self, p: &mut [u8], off: u64) -> (usize, Option<GoError>);
}

impl<T: ReaderAt + ?Sized> ReaderAt for &T {
    fn read_at(&self, p: &mut [u8], off: u64) -> (usize, Option<GoError>) {
        (**self).read_at(p, off)
    }
}

/// CTR 密码上下文：随机密钥/nonce 与加密块大小配置。
#[derive(Clone, Debug)]
pub struct CtrCipher {
    key: [u8; 16],
    nonce: u64,
    encryptBlockSize: i64,
    aesBlockCount: i64,
}

/// 使用默认块大小创建 `CtrCipher`。
pub fn NewCtrCipher() -> io::Result<CtrCipher> {
    NewCtrCipherWithBlockSize(defaultEncryptBlockSize)
}

/// 按指定块大小创建 `CtrCipher`；块大小须为正且为 16 的倍数。
pub fn NewCtrCipherWithBlockSize(encryptBlockSize: i64) -> io::Result<CtrCipher> {
    if encryptBlockSize <= 0 || encryptBlockSize % 16 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            errInvalidBlockSize,
        ));
    }
    let mut key = [0; 16];
    rand::rngs::OsRng.fill_bytes(&mut key);
    Ok(CtrCipher {
        key,
        nonce: rand::rngs::OsRng.next_u64() & i64::MAX as u64,
        encryptBlockSize,
        aesBlockCount: encryptBlockSize / 16,
    })
}

impl CtrCipher {
    /// 由 nonce 与计数器构造 CTR 流密码实例。
    fn stream(&self, counter: u64) -> ctr::Ctr128BE<Aes128> {
        let mut iv = [0; 16];
        // IV 高 8 字节为 nonce，低 8 字节为大端计数器。
        iv[..8].copy_from_slice(&self.nonce.to_be_bytes());
        iv[8..].copy_from_slice(&counter.to_be_bytes());
        ctr::Ctr128BE::<Aes128>::new(&self.key.into(), &iv.into())
    }
}

/// 缓冲式 CTR 加密写入器：填满块后密钥流变换再刷到底层 `WriteCloser`。
pub struct Writer<W: WriteCloser> {
    err: Option<GoError>,
    w: W,
    cipherStream: ctr::Ctr128BE<Aes128>,
    buf: Vec<u8>,
    flushedUserDataCnt: i64,
    n: usize,
}

/// 包装底层写入端并绑定 `CtrCipher`（从计数器 0 开始）。
pub fn NewWriter<W: WriteCloser>(w: W, cipher: &CtrCipher) -> Writer<W> {
    Writer {
        err: None,
        w,
        cipherStream: cipher.stream(0),
        buf: vec![0; cipher.encryptBlockSize as usize],
        flushedUserDataCnt: 0,
        n: 0,
    }
}

impl<W: WriteCloser> Writer<W> {
    /// 当前缓冲剩余可写字节数。
    pub fn AvailableSize(&self) -> usize {
        self.buf.len() - self.n
    }
    /// 当前已缓冲、尚未 Flush 的字节数。
    pub fn Buffered(&self) -> usize {
        self.n
    }
    /// 返回尚未刷出的明文缓冲切片。
    pub fn GetCache(&self) -> &[u8] {
        &self.buf[..self.n]
    }
    /// 已成功刷到底层的用户数据偏移（字节计数）。
    pub fn GetCacheDataOffset(&self) -> i64 {
        self.flushedUserDataCnt
    }

    /// 写入明文：缓冲满则加密 Flush，粘性错误会持续返回。
    pub fn Write(&mut self, mut p: &[u8]) -> (usize, Option<GoError>) {
        if let Some(err) = &self.err {
            return (0, Some(err.clone()));
        }
        let mut total = 0;
        while p.len() > self.AvailableSize() && self.err.is_none() {
            let copied = self.AvailableSize();
            self.buf[self.n..self.n + copied].copy_from_slice(&p[..copied]);
            self.n += copied;
            if let Some(err) = self.Flush() {
                return (total, Some(err));
            }
            total += copied;
            p = &p[copied..];
        }
        if let Some(err) = &self.err {
            return (total, Some(err.clone()));
        }
        self.buf[self.n..self.n + p.len()].copy_from_slice(p);
        self.n += p.len();
        (total + p.len(), None)
    }

    /// 对当前缓冲做 CTR 密钥流变换并写入底层；短写视为错误并粘住。
    pub fn Flush(&mut self) -> Option<GoError> {
        if let Some(err) = &self.err {
            return Some(err.clone());
        }
        if self.n == 0 {
            return None;
        }
        self.cipherStream.apply_keystream(&mut self.buf[..self.n]);
        let (written, mut err) = self.w.write(&self.buf[..self.n]);
        self.flushedUserDataCnt += written as i64;
        if written < self.n && err.is_none() {
            err = Some("short write".to_owned());
        }
        if let Some(err) = err {
            self.err = Some(err.clone());
            return Some(err);
        }
        self.n = 0;
        None
    }

    /// Flush 剩余缓冲后关闭底层写入端。
    pub fn Close(&mut self) -> Option<GoError> {
        if let Some(err) = self.Flush() {
            return Some(err);
        }
        self.w.close()
    }
}

impl<W: WriteCloser> WriteCloser for Writer<W> {
    fn write(&mut self, p: &[u8]) -> (usize, Option<GoError>) {
        self.Write(p)
    }

    fn close(&mut self) -> Option<GoError> {
        self.Close()
    }
}

/// CTR 解密读取器：按用户数据偏移定位计数器并解密。
pub struct Reader<R: ReaderAt> {
    r: R,
    cipher: CtrCipher,
}
/// 包装底层 `ReaderAt` 与 `CtrCipher`。
pub fn NewReader<R: ReaderAt>(r: R, cipher: CtrCipher) -> Reader<R> {
    Reader { r, cipher }
}

impl<R: ReaderAt> Reader<R> {
    /// 从用户数据偏移 `off` 起解密读入 `p`（负偏移报错）。
    pub fn ReadAt(&self, mut p: &mut [u8], off: i64) -> (usize, Option<GoError>) {
        if p.is_empty() {
            return (0, None);
        }
        if off < 0 {
            return (0, Some("negative offset".to_owned()));
        }
        let mut offset = (off % self.cipher.encryptBlockSize) as usize;
        // 将用户偏移映射到 CTR 计数器：整块数 × 每块 AES 块数。
        let counter = (off / self.cipher.encryptBlockSize) * self.cipher.aesBlockCount;
        let mut cursor = (off - offset as i64) as u64;
        let mut stream = self.cipher.stream(counter as u64);
        let mut buf = vec![0; self.cipher.encryptBlockSize as usize];
        let mut total = 0;
        while !p.is_empty() {
            let (read, read_err) = self.r.read_at(&mut buf, cursor);
            if let Some(read_err) = read_err {
                if read == 0 || read_err != "EOF" {
                    return (total, Some(read_err));
                }
                // 与 Go 一致：部分读取伴随 EOF 时，仍需处理本轮读到的数据。
            }
            if read == 0 {
                return (total, None);
            }
            cursor += read as u64;
            // 先对读出的密文块应用密钥流，再按块内偏移拷贝明文。
            stream.apply_keystream(&mut buf[..read]);
            if offset > read {
                return (total, Some("invalid reader offset".to_owned()));
            }
            let copied = p.len().min(read - offset);
            p[..copied].copy_from_slice(&buf[offset..offset + copied]);
            p = &mut p[copied..];
            total += copied;
            offset = 0;
        }
        (total, None)
    }
}

impl<R: ReaderAt> ReaderAt for Reader<R> {
    fn read_at(&self, p: &mut [u8], off: u64) -> (usize, Option<GoError>) {
        self.ReadAt(p, off as i64)
    }
}
