// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Copyright 2013 The Go-MySQL-Driver Authors. All rights reserved.

// MySQL 线协议 PacketIO：读写普通包与可选压缩包。
//
// MySQL 协议以 4 字节头（3 字节长度 + 1 字节序号）封装载荷；超过
// `MAX_PAYLOAD_LEN`（16MiB-1）时分包。开启压缩后外层另有 7 字节压缩头，
// 普通包序号与压缩包序号独立维护（与 Go 一致）。

use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use std::io::{self, Read, Write};
use std::time::Duration;

/// 写缓冲默认容量（16 KiB）。
pub const DEFAULT_WRITER_SIZE: usize = 16 * 1024;
/// 单个 MySQL 包载荷最大长度（0xFFFFFF，约 16MiB-1）。
pub const MAX_PAYLOAD_LEN: usize = 0x00ff_ffff;
/// 未启用压缩。
pub const COMPRESSION_NONE: i32 = 0;
/// zlib（deflate）压缩算法标识。
pub const COMPRESSION_ZLIB: i32 = 1;
/// zstd 压缩算法标识。
pub const COMPRESSION_ZSTD: i32 = 2;

/// 单次压缩缓冲上限，满则刷出一个压缩包。
const MAX_COMPRESSED_SIZE: usize = 1024 * 1024;
/// 短于该长度的载荷不压缩，直接放入压缩协议帧（未压缩长度为 0）。
const MIN_COMPRESS_LENGTH: usize = 50;

/// Reads and writes MySQL packets while maintaining the normal and compressed
/// protocol sequence numbers.
/// 读写 MySQL 协议包，并维护普通包与压缩包两套序号。
pub struct PacketIO {
    reader: Option<Box<dyn Read + Send>>,
    output: Vec<u8>,
    compressed_writer: Option<CompressedWriter>,
    compressed_reader: Option<CompressedReader<Box<dyn Read + Send>>>,
    read_timeout: Duration,
    max_allowed_packet: u64,
    accumulated_length: u64,
    compression_algorithm: i32,
    zstd_level: i32,
    sequence: u8,
    compressed_sequence: u8,
}

impl PacketIO {
    /// 从可读连接构造 PacketIO，并设置单次读累计长度上限。
    pub fn new_packet_io(reader: Box<dyn Read + Send>, max_allowed_packet: u64) -> Self {
        Self {
            reader: Some(reader),
            output: Vec::with_capacity(DEFAULT_WRITER_SIZE),
            compressed_writer: None,
            compressed_reader: None,
            read_timeout: Duration::ZERO,
            max_allowed_packet,
            accumulated_length: 0,
            compression_algorithm: COMPRESSION_NONE,
            zstd_level: 3,
            sequence: 0,
            compressed_sequence: 0,
        }
    }

    /// 测试用构造：无 reader，仅向内存 output 写包。
    pub fn new_packet_io_for_test(output: Vec<u8>) -> Self {
        Self {
            reader: None,
            output,
            compressed_writer: None,
            compressed_reader: None,
            read_timeout: Duration::ZERO,
            max_allowed_packet: 0,
            accumulated_length: 0,
            compression_algorithm: COMPRESSION_NONE,
            zstd_level: 3,
            sequence: 0,
            compressed_sequence: 0,
        }
    }

    /// 设置 zstd 压缩级别（传给 CompressedWriter）。
    pub fn set_zstd_level(&mut self, level: i32) {
        self.zstd_level = level;
    }

    /// 当前普通包序号。
    pub fn sequence(&self) -> u8 {
        self.sequence
    }

    /// 设置普通包序号（握手切换等场景需要重置）。
    pub fn set_sequence(&mut self, sequence: u8) {
        self.sequence = sequence;
    }

    /// 当前外层压缩包序号。
    pub fn compressed_sequence(&self) -> u8 {
        self.compressed_sequence
    }

    /// 设置外层压缩包序号。
    pub fn set_compressed_sequence(&mut self, sequence: u8) {
        self.compressed_sequence = sequence;
    }

    /// 替换写缓冲。
    pub fn set_buf_writer(&mut self, output: Vec<u8>) {
        self.output = output;
    }

    /// 重置写缓冲（语义同 set_buf_writer，对齐 Go Reset）。
    pub fn reset_buf_writer(&mut self, output: Vec<u8>) {
        self.output = output;
    }

    /// 切换底层读连接，并清空压缩读侧与写缓冲。
    pub fn set_buffered_read_conn(&mut self, reader: Box<dyn Read + Send>) {
        self.reader = Some(reader);
        self.compressed_reader = None;
        self.output.clear();
        self.output.reserve(DEFAULT_WRITER_SIZE);
    }

    /// 设置读超时（迁移占位；具体超时由上层连接实现）。
    pub fn set_read_timeout(&mut self, timeout: Duration) {
        self.read_timeout = timeout;
    }

    /// 设置单次 `read_packet` 累计载荷上限（对齐 max_allowed_packet）。
    pub fn set_max_allowed_packet(&mut self, max_allowed_packet: u64) {
        self.max_allowed_packet = max_allowed_packet;
    }

    /// Enables MySQL protocol compression. As in Go, the ordinary packet and
    /// outer compressed packet sequence numbers are tracked independently.
    /// 启用 MySQL 协议压缩；普通包与外层压缩包序号独立跟踪（与 Go 一致）。
    pub fn set_compression_algorithm(&mut self, algorithm: i32) -> Result<(), String> {
        if algorithm != COMPRESSION_ZLIB && algorithm != COMPRESSION_ZSTD {
            return Err("unknown compression algorithm".into());
        }
        self.compression_algorithm = algorithm;
        self.compressed_writer = Some(CompressedWriter::new(
            algorithm,
            self.compressed_sequence,
            self.zstd_level,
        ));
        // 若已有 reader，包装为 CompressedReader 以解码外层压缩帧。
        if let Some(reader) = self.reader.take() {
            self.compressed_reader = Some(CompressedReader::new(
                reader,
                algorithm,
                self.compressed_sequence,
            ));
        }
        Ok(())
    }

    /// 读取一个物理包（4 字节头 + 载荷），校验序号并累计长度。
    fn read_one_packet(&mut self) -> Result<Vec<u8>, String> {
        let mut header = [0_u8; 4];
        self.read_exact_protocol(&mut header)?;
        let length = read_u24(&header[..3]);
        // 未压缩时严格校验序号；压缩开启后内层序号可忽略（兼容部分驱动）。
        if header[3] != self.sequence && self.compression_algorithm == COMPRESSION_NONE {
            return Err(format!(
                "invalid sequence, received {} while expecting {}",
                header[3], self.sequence
            ));
        }
        self.sequence = self.sequence.wrapping_add(1);
        self.accumulated_length = self.accumulated_length.saturating_add(length as u64);
        if self.accumulated_length > self.max_allowed_packet {
            return Err("packet too large".into());
        }

        let mut data = vec![0; length];
        self.read_exact_protocol(&mut data)?;
        Ok(data)
    }

    /// 按是否压缩选择从 reader 或 CompressedReader 精确读满缓冲区。
    fn read_exact_protocol(&mut self, data: &mut [u8]) -> Result<(), String> {
        if self.compression_algorithm == COMPRESSION_NONE {
            self.reader
                .as_mut()
                .ok_or_else(|| "missing reader".to_owned())?
                .read_exact(data)
                .map_err(|error| error.to_string())
        } else {
            let reader = self
                .compressed_reader
                .as_mut()
                .ok_or_else(|| "missing compressed reader".to_owned())?;
            reader.read_exact(data).map_err(|error| error.to_string())?;
            self.compressed_sequence = reader.sequence();
            Ok(())
        }
    }

    /// 读取完整逻辑包：若载荷恰为 MAX_PAYLOAD_LEN 则继续拼接后续分包。
    pub fn read_packet(&mut self) -> Result<Vec<u8>, String> {
        self.accumulated_length = 0;
        let mut data = self.read_one_packet()?;
        // MySQL 分包规则：满长包后继续读，直到读到更短的一包结束。
        while data.len() == MAX_PAYLOAD_LEN {
            let next = self.read_one_packet()?;
            let finished = next.len() < MAX_PAYLOAD_LEN;
            data.extend_from_slice(&next);
            if finished {
                break;
            }
        }
        Ok(data)
    }

    /// Writes data which already reserves its first four bytes for a MySQL
    /// packet header. Large payloads use the same four-byte overlap as Go.
    /// 写入已预留前 4 字节包头的数据；超大载荷按 Go 同款四字节重叠分包。
    pub fn write_packet(&mut self, mut data: &mut [u8]) -> Result<(), String> {
        if data.len() < 4 {
            return Err("malformed packet".into());
        }
        let mut length = data.len() - 4;
        // 满长分包：长度写 0xFFFFFF，序号写入后推进，切片前移 MAX_PAYLOAD_LEN。
        while length >= MAX_PAYLOAD_LEN {
            data[..3].copy_from_slice(&[0xff, 0xff, 0xff]);
            data[3] = self.sequence;
            self.write_protocol(&data[..4 + MAX_PAYLOAD_LEN])?;
            self.sequence = self.sequence.wrapping_add(1);
            length -= MAX_PAYLOAD_LEN;
            data = &mut data[MAX_PAYLOAD_LEN..];
        }

        write_u24(&mut data[..3], length);
        data[3] = self.sequence;
        self.write_protocol(data)?;
        self.sequence = self.sequence.wrapping_add(1);
        Ok(())
    }

    /// 未压缩时追加到 output；压缩时交给 CompressedWriter。
    fn write_protocol(&mut self, data: &[u8]) -> Result<(), String> {
        if self.compression_algorithm == COMPRESSION_NONE {
            self.output.extend_from_slice(data);
            Ok(())
        } else {
            self.compressed_writer
                .as_mut()
                .ok_or_else(|| "missing compressed writer".to_owned())?
                .write_all(data)
                .map_err(|error| error.to_string())
        }
    }

    /// 刷出压缩侧待写数据到 output，并同步两套序号。
    pub fn flush(&mut self) -> Result<(), String> {
        if let Some(writer) = self.compressed_writer.as_mut() {
            self.output
                .extend_from_slice(&writer.flush().map_err(|error| error.to_string())?);
            self.compressed_sequence = writer.sequence();
            // 对齐 Go：flush 后普通序号跟压缩序号对齐。
            self.sequence = self.compressed_sequence;
        }
        Ok(())
    }

    /// Reset both protocol sequences at a command boundary while retaining the
    /// selected compression algorithm and the underlying reader.
    pub fn reset_sequence(&mut self) {
        self.sequence = 0;
        self.compressed_sequence = 0;
        if self.compression_algorithm != COMPRESSION_NONE {
            self.compressed_writer = Some(CompressedWriter::new(
                self.compression_algorithm,
                0,
                self.zstd_level,
            ));
            if let Some(reader) = self.compressed_reader.as_mut() {
                reader.reset_sequence();
            }
        }
    }

    /// 查看当前已写出的字节（测试与上层取缓冲）。
    pub fn written_data(&self) -> &[u8] {
        &self.output
    }

    /// Safely take all fully encoded output while keeping PacketIO reusable.
    /// Socket transports call this only after `flush`, so compressed pending
    /// bytes have already joined this buffer.
    pub fn take_written_data(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    /// 消费 self，返回写出缓冲所有权。
    pub fn into_written_data(self) -> Vec<u8> {
        self.output
    }
}

/// 便捷构造：同 `PacketIO::new_packet_io`。
pub fn new_packet_io(reader: Box<dyn Read + Send>, max_allowed_packet: u64) -> PacketIO {
    PacketIO::new_packet_io(reader, max_allowed_packet)
}

/// 便捷构造：同 `PacketIO::new_packet_io_for_test`。
pub fn new_packet_io_for_test(output: Vec<u8>) -> PacketIO {
    PacketIO::new_packet_io_for_test(output)
}

/// Buffers ordinary packet bytes and emits MySQL compressed protocol packets.
/// 缓冲普通包字节并输出 MySQL 压缩协议帧（7 字节头 + 载荷）。
pub struct CompressedWriter {
    pending: Vec<u8>,
    encoded: Vec<u8>,
    sequence: u8,
    algorithm: i32,
    zstd_level: i32,
}

impl CompressedWriter {
    /// 创建压缩写端；`sequence` 为起始外层序号。
    pub fn new(algorithm: i32, sequence: u8, zstd_level: i32) -> Self {
        Self {
            pending: Vec::new(),
            encoded: Vec::new(),
            sequence,
            algorithm,
            zstd_level,
        }
    }

    /// 当前外层压缩序号。
    pub fn sequence(&self) -> u8 {
        self.sequence
    }

    /// 刷出最后一个压缩包并取出全部已编码字节。
    pub fn flush(&mut self) -> io::Result<Vec<u8>> {
        self.flush_packet()?;
        Ok(std::mem::take(&mut self.encoded))
    }

    /// 将 pending 编码为一个压缩协议包追加到 encoded。
    fn flush_packet(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.pending);
        // 短载荷不压缩：payload 原样，uncompressed_length=0。
        let (payload, uncompressed_length) = if data.len() > MIN_COMPRESS_LENGTH {
            (
                compress(&data, self.algorithm, self.zstd_level)?,
                data.len(),
            )
        } else {
            (data, 0)
        };
        if payload.len() > MAX_PAYLOAD_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "compressed packet too large",
            ));
        }
        // 压缩头：3 字节压缩长度 + 1 字节序号 + 3 字节未压缩长度。
        let mut header = [0_u8; 7];
        write_u24(&mut header[..3], payload.len());
        header[3] = self.sequence;
        write_u24(&mut header[4..7], uncompressed_length);
        self.encoded.extend_from_slice(&header);
        self.encoded.extend_from_slice(&payload);
        self.sequence = self.sequence.wrapping_add(1);
        Ok(())
    }
}

impl Write for CompressedWriter {
    fn write(&mut self, mut data: &[u8]) -> io::Result<usize> {
        let original_length = data.len();
        // 按 MAX_COMPRESSED_SIZE 切分 pending，满则刷包。
        while !data.is_empty() {
            let available = MAX_COMPRESSED_SIZE - self.pending.len();
            let count = available.min(data.len());
            self.pending.extend_from_slice(&data[..count]);
            data = &data[count..];
            if self.pending.len() == MAX_COMPRESSED_SIZE {
                self.flush_packet()?;
            }
        }
        Ok(original_length)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_packet()
    }
}

/// Decodes MySQL compressed protocol packets and exposes their ordinary packet
/// bytes through `Read`.
/// 解码 MySQL 压缩协议包，通过 `Read` 暴露解压后的普通包字节流。
pub struct CompressedReader<R> {
    inner: R,
    sequence: u8,
    algorithm: i32,
    decoded: Vec<u8>,
    position: usize,
}

impl<R: Read> CompressedReader<R> {
    /// 包装底层 reader；`sequence` 为期望的起始外层序号。
    pub fn new(inner: R, algorithm: i32, sequence: u8) -> Self {
        Self {
            inner,
            sequence,
            algorithm,
            decoded: Vec::new(),
            position: 0,
        }
    }

    /// 当前外层压缩序号（读完一帧后已推进）。
    pub fn sequence(&self) -> u8 {
        self.sequence
    }

    /// Reset the expected outer sequence at a command boundary.
    pub fn reset_sequence(&mut self) {
        self.sequence = 0;
        self.decoded.clear();
        self.position = 0;
    }

    /// 从底层读取并解码一帧压缩包到 `decoded`；EOF 返回 false。
    fn load_packet(&mut self) -> io::Result<bool> {
        let mut header = [0_u8; 7];
        if self.inner.read(&mut header[..1])? == 0 {
            return Ok(false);
        }
        self.inner.read_exact(&mut header[1..])?;
        let compressed_length = read_u24(&header[..3]);
        if header[3] != self.sequence {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid compressed sequence, received {} while expecting {}",
                    header[3], self.sequence
                ),
            ));
        }
        self.sequence = self.sequence.wrapping_add(1);
        let uncompressed_length = read_u24(&header[4..7]);
        let mut payload = vec![0; compressed_length];
        self.inner.read_exact(&mut payload)?;
        // uncompressed_length==0 表示载荷未压缩，直接使用。
        self.decoded = if uncompressed_length == 0 {
            payload
        } else {
            decompress(&payload, self.algorithm, uncompressed_length)?
        };
        if uncompressed_length != 0 && self.decoded.len() != uncompressed_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "uncompressed packet length mismatch",
            ));
        }
        self.position = 0;
        Ok(true)
    }
}

impl<R: Read> Read for CompressedReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        // 当前帧读尽则加载下一压缩帧。
        if self.position == self.decoded.len() {
            if !self.load_packet()? {
                return Ok(0);
            }
        }
        let count = output
            .len()
            .min(self.decoded.len().saturating_sub(self.position));
        output[..count].copy_from_slice(&self.decoded[self.position..self.position + count]);
        self.position += count;
        if self.position == self.decoded.len() {
            self.decoded.clear();
            self.position = 0;
        }
        Ok(count)
    }
}

/// 按算法压缩数据（zlib 或 zstd）。
fn compress(data: &[u8], algorithm: i32, zstd_level: i32) -> io::Result<Vec<u8>> {
    match algorithm {
        COMPRESSION_ZLIB => {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(data)?;
            encoder.finish()
        }
        COMPRESSION_ZSTD => zstd::bulk::compress(data, zstd_level).map_err(io::Error::other),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown compression algorithm",
        )),
    }
}

/// 按算法解压；`expected_length` 用于 zstd 输出容量与长度校验。
fn decompress(data: &[u8], algorithm: i32, expected_length: usize) -> io::Result<Vec<u8>> {
    match algorithm {
        COMPRESSION_ZLIB => {
            let decoder = ZlibDecoder::new(data);
            // Go decodes into a fixed `uncompressedLength` buffer. Read at most
            // one byte beyond that advertised boundary so a forged zlib stream
            // cannot grow an unbounded Vec before the length check runs.
            let mut decoder = decoder.take(expected_length.saturating_add(1) as u64);
            let mut output = Vec::with_capacity(expected_length);
            decoder.read_to_end(&mut output)?;
            if output.len() > expected_length {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "decompressed packet exceeds advertised length",
                ));
            }
            Ok(output)
        }
        COMPRESSION_ZSTD => zstd::bulk::decompress(data, expected_length).map_err(io::Error::other),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown compression algorithm",
        )),
    }
}

/// 小端读取 3 字节无符号整数。
fn read_u24(bytes: &[u8]) -> usize {
    bytes[0] as usize | (bytes[1] as usize) << 8 | (bytes[2] as usize) << 16
}

/// 小端写入 3 字节无符号整数。
fn write_u24(bytes: &mut [u8], value: usize) {
    bytes[0] = value as u8;
    bytes[1] = (value >> 8) as u8;
    bytes[2] = (value >> 16) as u8;
}

#[cfg(test)]
#[path = "packetio_test.rs"]
mod packetio_test;
