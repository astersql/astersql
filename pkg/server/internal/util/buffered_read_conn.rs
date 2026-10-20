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

#[cfg(unix)]
use std::os::fd::AsRawFd;

/// Default size of the buffered reader, matching Go's `DefaultReaderSize`.
/// 默认读缓冲大小（16KiB），与 Go 的 `DefaultReaderSize` 一致。
pub const DefaultReaderSize: usize = 16 * 1024;

/// A TCP connection whose reads pass through a buffered reader.
/// 读路径经过 `BufReader` 的 TCP 连接包装。
pub struct BufferedReadConn {
    /// 读路径的独立缓冲状态；探活从不触碰该锁或 `BufReader`。
    reader: Mutex<BufferedReaderState>,
    /// 从包装层中显式取出的底层 TCP socket，仅用于非消费探测。
    probe_stream: Option<TcpStream>,
}

struct BufferedReaderState {
    reader: BufReader<Box<dyn BufferedReadTransport>>,
    peeked: Vec<u8>,
}

/// A readable connection wrapper that can explicitly expose its underlying TCP
/// socket. This is the Rust counterpart of Go's `NetConn() net.Conn` contract.
pub trait BufferedReadTransport: Read + Send {
    fn net_stream(&self) -> Option<&TcpStream> {
        None
    }
}

impl BufferedReadTransport for TcpStream {
    fn net_stream(&self) -> Option<&TcpStream> {
        Some(self)
    }
}

/// Creates a buffered connection while retaining a cloned handle to the
/// explicitly exposed underlying socket for non-consuming liveness probes.
/// 用默认缓冲容量包装读路径，探活只通过显式暴露的底层 socket 执行。
pub fn NewBufferedReadConn<T>(stream: T) -> io::Result<BufferedReadConn>
where
    T: BufferedReadTransport + 'static,
{
    let probe_stream = stream.net_stream().map(TcpStream::try_clone).transpose()?;
    Ok(BufferedReadConn {
        reader: Mutex::new(BufferedReaderState {
            reader: BufReader::with_capacity(DefaultReaderSize, Box::new(stream)),
            peeked: Vec::new(),
        }),
        probe_stream,
    })
}

impl BufferedReadConn {
    /// Returns the next `n` bytes without consuming them.
    /// 窥视接下来的 `n` 字节且不消费；`n` 不得超过缓冲容量。
    pub fn Peek(&self, n: usize) -> io::Result<Vec<u8>> {
        let mut state = self
            .reader
            .lock()
            .map_err(|_| io::Error::other("buffered reader lock poisoned"))?;
        if n > state.reader.capacity() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "peek size exceeds reader capacity",
            ));
        }

        // `BufRead::fill_buf` 只保证至少进行一次底层读取。TCP 数据可能分片到达，
        // 因此像 Go 的 `bufio.Reader.Peek` 一样持续填充，直到取得 n 字节或遇到错误。
        while state.peeked.len() < n {
            let remaining = n - state.peeked.len();
            let buffered = state.reader.fill_buf()?;
            if buffered.is_empty() {
                return Err(ErrorKind::UnexpectedEof.into());
            }
            let copied = remaining.min(buffered.len());
            let bytes = buffered[..copied].to_vec();
            state.reader.consume(copied);
            state.peeked.extend_from_slice(&bytes);
        }
        Ok(state.peeked[..n].to_vec())
    }

    /// Returns 0 for EOF, 1 for a live idle connection, and -1 when the probe
    /// cannot determine liveness, matching the Go implementation.
    /// 探活：0=对端已关闭(EOF)，1=空闲但仍存活，-1=无法判定。
    pub fn IsAlive(&self) -> i32 {
        self.probe_stream
            .as_ref()
            .map(probe_tcp_stream_alive)
            .unwrap_or(-1)
    }

    /// Reads through the buffered path while permitting `IsAlive` to probe the
    /// socket concurrently without touching buffered state.
    pub fn Read(&self, output: &mut [u8]) -> io::Result<usize> {
        let mut state = self
            .reader
            .lock()
            .map_err(|_| io::Error::other("buffered reader lock poisoned"))?;
        if output.is_empty() {
            return Ok(0);
        }
        if !state.peeked.is_empty() {
            let copied = output.len().min(state.peeked.len());
            output[..copied].copy_from_slice(&state.peeked[..copied]);
            state.peeked.drain(..copied);
            return Ok(copied);
        }
        state.reader.read(output)
    }
}

/// Probes a TCP socket without consuming bytes or changing file-description
/// flags/read deadlines shared with the packet reader.
#[cfg(unix)]
pub fn probe_tcp_stream_alive(stream: &TcpStream) -> i32 {
    let mut byte = [0_u8; 1];
    let result = unsafe {
        libc::recv(
            stream.as_raw_fd(),
            byte.as_mut_ptr().cast(),
            byte.len(),
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    if result > 0 {
        return 1;
    }
    if result == 0 {
        return 0;
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => 1,
        Some(code)
            if code == libc::ECONNRESET
                || code == libc::EPIPE
                || code == libc::ENOTCONN
                || code == libc::ETIMEDOUT =>
        {
            0
        }
        _ => -1,
    }
}

#[cfg(not(unix))]
pub fn probe_tcp_stream_alive(_stream: &TcpStream) -> i32 {
    -1
}

impl Read for BufferedReadConn {
    /// 委托给内部 `BufReader` 读取。
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.Read(output)
    }
}
