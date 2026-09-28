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

// 服务端单测替身：内存字节连接与地址端口提取。
//
// `BytesConn` 以缓冲区模拟可读连接，写入与 deadline 操作为成功空操作，
// 对齐 Go 侧测试桩，便于 PacketIO 等协议层在无真实 socket 时做读写测试。

use std::io::{self, Cursor, Read, Write};
use std::net::SocketAddr;
use std::time::SystemTime;

/// An in-memory connection whose reads are backed by a byte buffer.
///
/// Like the Go test double, writes and connection-management operations are
/// successful no-ops.
/// 内存连接：读操作消费内部字节缓冲；写与连接管理均为成功空操作（对齐 Go 测试桩）。
#[derive(Debug, Default)]
pub struct BytesConn {
    /// 可读字节游标（Cursor 维护读偏移）。
    buffer: Cursor<Vec<u8>>,
}

impl BytesConn {
    /// 用给定字节缓冲构造内存连接。
    pub fn new(buffer: Vec<u8>) -> Self {
        Self {
            buffer: Cursor::new(buffer),
        }
    }

    /// 关闭连接（空操作，始终成功）。
    pub fn close(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// 本地地址（桩实现恒为 None）。
    pub fn local_addr(&self) -> Option<SocketAddr> {
        None
    }

    /// 对端地址（桩实现恒为 None）。
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }

    /// 设置读写 deadline（空操作）。
    pub fn set_deadline(&mut self, _deadline: SystemTime) -> io::Result<()> {
        Ok(())
    }

    /// 设置读 deadline（空操作）。
    pub fn set_read_deadline(&mut self, _deadline: SystemTime) -> io::Result<()> {
        Ok(())
    }

    /// 设置写 deadline（空操作）。
    pub fn set_write_deadline(&mut self, _deadline: SystemTime) -> io::Result<()> {
        Ok(())
    }
}

impl From<Vec<u8>> for BytesConn {
    fn from(buffer: Vec<u8>) -> Self {
        Self::new(buffer)
    }
}

impl Read for BytesConn {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.buffer.read(output)
    }
}

impl Write for BytesConn {
    /// 丢弃写入内容，返回写入长度为 0（与 Go 桩一致）。
    fn write(&mut self, _input: &[u8]) -> io::Result<usize> {
        Ok(0)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Gets the port from a TCP socket address.
/// 从 TCP `SocketAddr` 提取端口号。
pub fn get_port_from_tcp_addr(addr: SocketAddr) -> u16 {
    addr.port()
}
