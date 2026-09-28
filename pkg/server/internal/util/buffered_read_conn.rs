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

// 带缓冲的 TCP 读连接封装。
//
// 在 `BufReader` 之上提供 Peek（窥视未消费字节）与 IsAlive（空闲连接探活），
// 对齐 Go 侧 `BufferedReadConn`，供 MySQL 协议层在解析命令包时复用已读缓冲。

use std::io::{self, BufRead, BufReader, ErrorKind, Read};
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::Duration;

/// Default size of the buffered reader, matching Go's `DefaultReaderSize`.
/// 默认读缓冲大小（16KiB），与 Go 的 `DefaultReaderSize` 一致。
pub const DefaultReaderSize: usize = 16 * 1024;

/// A TCP connection whose reads pass through a buffered reader.
/// 读路径经过 `BufReader` 的 TCP 连接包装。
pub struct BufferedReadConn {
    /// 底层带缓冲的读端。
    reader: BufReader<TcpStream>,
    /// 已由 `Peek` 从 `BufReader` 暂存、但尚未被 `Read` 消费的字节。
    peeked: Vec<u8>,
    /// 探活互斥锁：避免并发 `IsAlive` 互相干扰读超时设置。
    liveness_lock: Mutex<()>,
}

/// Creates a buffered connection while retaining access to the underlying
/// stream for read-deadline based liveness probes.
/// 用默认缓冲容量包装 `TcpStream`；底层流仍可通过 `get_ref` 设置读超时做探活。
pub fn NewBufferedReadConn(stream: TcpStream) -> io::Result<BufferedReadConn> {
    Ok(BufferedReadConn {
        reader: BufReader::with_capacity(DefaultReaderSize, stream),
        peeked: Vec::new(),
        liveness_lock: Mutex::new(()),
    })
}

impl BufferedReadConn {
    /// Returns the next `n` bytes without consuming them.
    /// 窥视接下来的 `n` 字节且不消费；`n` 不得超过缓冲容量。
    pub fn Peek(&mut self, n: usize) -> io::Result<&[u8]> {
        if n > self.reader.capacity() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "peek size exceeds reader capacity",
            ));
        }

        // `BufRead::fill_buf` 只保证至少进行一次底层读取。TCP 数据可能分片到达，
        // 因此像 Go 的 `bufio.Reader.Peek` 一样持续填充，直到取得 n 字节或遇到错误。
        while self.peeked.len() < n {
            let buffered = self.reader.fill_buf()?;
            if buffered.is_empty() {
                return Err(ErrorKind::UnexpectedEof.into());
            }
            let copied = (n - self.peeked.len()).min(buffered.len());
            self.peeked.extend_from_slice(&buffered[..copied]);
            self.reader.consume(copied);
        }
        Ok(&self.peeked[..n])
    }

    /// Returns 0 for EOF, 1 for a live idle connection, and -1 when the probe
    /// cannot determine liveness, matching the Go implementation.
    /// 探活：0=对端已关闭(EOF)，1=空闲但仍存活，-1=无法判定（锁冲突/超时设置失败等）。
    pub fn IsAlive(&mut self) -> i32 {
        // try_lock 失败说明已有探活在进行，与 Go 一样返回 -1。
        let Ok(_guard) = self.liveness_lock.try_lock() else {
            return -1;
        };
        // 极短读超时：无数据时 TimedOut/WouldBlock 表示连接仍活。
        if self
            .reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_micros(30)))
            .is_err()
        {
            return -1;
        }

        let result = if !self.peeked.is_empty() {
            -1
        } else {
            match self.reader.fill_buf() {
                Ok(buffered) if buffered.is_empty() => 0,
                Err(error)
                    if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) =>
                {
                    1
                }
                _ => -1,
            }
        };
        // 探活结束清除读超时，恢复正常阻塞读语义。
        let _ = self.reader.get_ref().set_read_timeout(None);
        result
    }
}

impl Read for BufferedReadConn {
    /// 委托给内部 `BufReader` 读取。
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if !self.peeked.is_empty() {
            let copied = output.len().min(self.peeked.len());
            output[..copied].copy_from_slice(&self.peeked[..copied]);
            self.peeked.drain(..copied);
            return Ok(copied);
        }
        self.reader.read(output)
    }
}
