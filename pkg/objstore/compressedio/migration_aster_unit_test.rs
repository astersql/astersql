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

// compressedio 迁移对照测试：压缩类型解析、缓冲往返与无压缩工厂语义对齐 Go。

use std::io::{Cursor, Read, Write};

use super::*;

/// 校验 `parse_compress_type` 与文件后缀表与 Go 一致，并覆盖未知类型报错。
#[test]
fn parse_compress_type_and_suffix_match_go() {
    let cases = [
        ("", CompressType::NoCompression, ""),
        ("no-compression", CompressType::NoCompression, ""),
        ("gzip", CompressType::Gzip, ".gz"),
        ("gz", CompressType::Gzip, ".gz"),
        ("snappy", CompressType::Snappy, ".snappy"),
        ("zstd", CompressType::Zstd, ".zst"),
        ("zst", CompressType::Zstd, ".zst"),
    ];

    for (input, expected, suffix) in cases {
        let parsed = parse_compress_type(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(parsed.file_suffix(), suffix);
    }

    let err = parse_compress_type("brotli").unwrap_err();
    assert_eq!(err.to_string(), "unknown compress type brotli");
}

/// 对 Gzip / Snappy / Zstd：写入缓冲后关闭，再经 `new_reader` 解压应还原原文。
#[test]
fn compressed_buffer_round_trips_all_go_formats() {
    let input = b"hello compressed world".repeat(128);

    for compress_type in [CompressType::Gzip, CompressType::Snappy, CompressType::Zstd] {
        let mut buffer = new_buffer(64, compress_type);
        assert_eq!(buffer.cap(), 64);
        assert!(buffer.compressed());
        buffer.write_all(&input).unwrap();
        buffer.flush().unwrap();
        assert!(buffer.len() > 0);
        buffer.close().unwrap();

        let compressed = buffer.bytes();
        assert_ne!(compressed, input);
        let mut reader = new_reader(
            compress_type,
            DecompressConfig::default(),
            Box::new(Cursor::new(compressed)),
        )
        .unwrap()
        .unwrap();
        let mut decoded = Vec::new();
        reader.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, input);
    }
}

/// `reset` 应清空可见输出但保留容量，便于复用同一缓冲继续写。
#[test]
fn buffer_reset_reuses_capacity_and_clears_visible_output() {
    let mut buffer = new_buffer(32, CompressType::Snappy);
    buffer.write_all(b"first chunk").unwrap();
    buffer.flush().unwrap();
    assert!(buffer.len() > 0);

    buffer.reset();
    assert_eq!(buffer.len(), 0);
    assert_eq!(buffer.cap(), 32);

    buffer.write_all(b"second chunk").unwrap();
    buffer.flush().unwrap();
    assert!(buffer.len() > 0);
}

/// Zstd 同步解码（并发度 1）路径应能正确往返。
#[test]
fn zstd_sync_decode_configuration_round_trips() {
    let input = b"zstd synchronous decode".repeat(64);
    let mut buffer = new_buffer(128, CompressType::Zstd);
    buffer.write_all(&input).unwrap();
    buffer.close().unwrap();

    let mut reader = new_reader(
        CompressType::Zstd,
        DecompressConfig {
            zstd_decode_concurrency: 1,
        },
        Box::new(Cursor::new(buffer.bytes())),
    )
    .unwrap()
    .unwrap();
    let mut decoded = Vec::new();
    reader.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, input);
}

/// 无压缩时工厂返回 None，对应 Go 侧返回 nil 的语义。
#[test]
fn no_compression_factories_match_go_nil_semantics() {
    assert!(new_writer(CompressType::NoCompression, Box::new(Vec::<u8>::new())).is_none());
    assert!(
        new_reader(
            CompressType::NoCompression,
            DecompressConfig::default(),
            Box::new(Cursor::new(Vec::<u8>::new())),
        )
        .unwrap()
        .is_none()
    );
}
