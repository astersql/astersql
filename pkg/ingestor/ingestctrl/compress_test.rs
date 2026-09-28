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

// Gzip 压缩/解压单元与基准风格测试。
//
// 验证自定义 `gzipCompressor`/`gzipDecompressor` 与池化标准库实现可互相编解码，
// 并保留与 Go 同名的 benchmark 结构（固定输入、循环压缩/解压）。

#![allow(dead_code)]
#![allow(non_snake_case)]

use std::io::{Cursor, Read};

use astersql_util_compress::{GzipReaderPool, GzipWriterPool};

use crate::compress::{gzipCompressor, gzipDecompressor};

/// 生成约 1MiB 可复现伪随机输入，供压缩往返与基准复用。
fn input_bytes() -> Vec<u8> {
    (0..1 << 20)
        .map(|index| ((index * 31 + index / 7) & 0xff) as u8)
        .collect()
}

// TestGzipCompressor 对应 Go 的同名测试：验证自定义 gzipCompressor 类型名和压缩结果可被标准库解开。
/// 验证自定义压缩器类型名，以及压缩结果可被池化标准 gzip 解开。
#[test]
pub fn TestGzipCompressor() {
    let compressor = gzipCompressor;
    assert_eq!("gzip", compressor.Type());
    let input = input_bytes();
    let mut compressed = Vec::new();
    compressor.Do(&mut compressed, &input).unwrap();
    let mut decoder = GzipReaderPool.get();
    decoder.reset(compressed);
    let mut uncompressed = Vec::new();
    decoder.read_to_end(&mut uncompressed).unwrap();
    GzipReaderPool.put(decoder);
    assert_eq!(input, uncompressed);
}

// TestGzipDecompressor 对应 Go 的同名测试：先用标准 gzip.Writer 造数据，再验证自定义解压器。
/// 用标准 gzip 造压缩数据，再验证自定义解压器往返一致。
#[test]
pub fn TestGzipDecompressor() {
    let decompressor = gzipDecompressor;
    assert_eq!("gzip", decompressor.Type());
    let input = input_bytes();
    let mut encoder = GzipWriterPool.get();
    let compressed = encoder.compress(&input).unwrap();
    GzipWriterPool.put(encoder);
    let uncompressed = decompressor.Do(&mut Cursor::new(compressed)).unwrap();
    assert_eq!(input, uncompressed);
}

// BenchmarkGzipCompressor 对应 Go benchmark，复用 benchCompressor 覆盖自定义压缩器。
/// 基准风格：对自定义 gzipCompressor 做多轮压缩。
#[test]
pub fn BenchmarkGzipCompressor() {
    benchCompressor(&gzipCompressor);
}

// BenchmarkGrpcGzipCompressor 对应 Go benchmark，用 gRPC 标准 gzip 压缩器作为对照。
/// 基准风格对照：直接使用池化标准 gzip 压缩器。
#[test]
pub fn BenchmarkGrpcGzipCompressor() {
    let input = input_bytes();
    let mut encoder = GzipWriterPool.get();
    assert!(!encoder.compress(&input).unwrap().is_empty());
    GzipWriterPool.put(encoder);
}

// benchCompressor 保留 Go 的随机输入、ResetTimer 和每轮重新创建 bytes.Buffer 的 benchmark 结构。
/// 对给定压缩器循环压入新 buffer，断言输出非空。
pub fn benchCompressor(compressor: &gzipCompressor) {
    let input = input_bytes();
    for _ in 0..4 {
        let mut output = Vec::new();
        compressor.Do(&mut output, &input).unwrap();
        assert!(!output.is_empty());
    }
}

// BenchmarkGzipDecompressor 对应 Go benchmark，覆盖自定义解压器。
/// 基准风格：对自定义 gzipDecompressor 做多轮解压。
#[test]
pub fn BenchmarkGzipDecompressor() {
    benchDecompressor(&gzipDecompressor);
}

// BenchmarkGrpcGzipDecompressor 对应 Go benchmark，用 gRPC 标准 gzip 解压器作为对照。
/// 基准风格对照：池化标准 gzip 解压后应与原始输入一致。
#[test]
pub fn BenchmarkGrpcGzipDecompressor() {
    let input = input_bytes();
    let mut encoder = GzipWriterPool.get();
    let compressed = encoder.compress(&input).unwrap();
    GzipWriterPool.put(encoder);
    let mut decoder = GzipReaderPool.get();
    decoder.reset(compressed);
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).unwrap();
    GzipReaderPool.put(decoder);
    assert_eq!(input, decoded);
}

// benchDecompressor 先在计时外准备 gzip 数据，再在循环内反复从新 reader 解压。
/// 先准备压缩载荷，再在循环内从新 Cursor 反复解压并比对。
pub fn benchDecompressor(decompressor: &gzipDecompressor) {
    let input = input_bytes();
    let mut encoder = GzipWriterPool.get();
    let compressed = encoder.compress(&input).unwrap();
    GzipWriterPool.put(encoder);
    for _ in 0..4 {
        assert_eq!(
            input,
            decompressor.Do(&mut Cursor::new(&compressed)).unwrap()
        );
    }
}
