// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// PacketIO 迁移对照单元测试。
//
// 覆盖普通包头重写与序号推进、序号/包大小校验、短/长压缩载荷往返，
// 以及压缩外层序号错误与内层序号忽略（对齐 Go / MariaDB Connector/J）。

use crate::{COMPRESSION_ZLIB, COMPRESSION_ZSTD, CompressedReader, CompressedWriter, PacketIO};
use std::io::{Cursor, Read, Write};

/// 写入时应回填 3 字节长度 + 序号到包头，并推进 sequence。
#[test]
fn packet_write_rewrites_header_and_advances_sequence() {
    let mut packet = PacketIO::new_packet_io_for_test(Vec::new());
    let mut data = vec![0, 0, 0, 0, 1, 2, 3];

    packet.write_packet(&mut data).unwrap();
    packet.flush().unwrap();

    assert_eq!(packet.written_data(), &[3, 0, 0, 0, 1, 2, 3]);
    assert_eq!(packet.sequence(), 1);
}

/// 读包时校验序号；累计长度超过 max_allowed_packet 应报 too large。
#[test]
fn packet_read_checks_sequence_and_max_allowed_packet() {
    let mut wrong_sequence =
        PacketIO::new_packet_io(Box::new(Cursor::new(vec![1, 0, 0, 1, 7])), 16);
    assert!(
        wrong_sequence
            .read_packet()
            .unwrap_err()
            .contains("sequence")
    );

    let mut too_large = PacketIO::new_packet_io(Box::new(Cursor::new(vec![2, 0, 0, 0, 7, 8])), 1);
    assert!(too_large.read_packet().unwrap_err().contains("too large"));
}

/// 短载荷不压缩：7 字节压缩头中未压缩长度为 0，载荷原样透传。
#[test]
fn compressed_short_payload_keeps_bytes_and_protocol_header() {
    let mut writer = CompressedWriter::new(COMPRESSION_ZLIB, 0, 3);
    writer.write_all(b"test_short").unwrap();
    let encoded = writer.flush().unwrap();

    assert_eq!(&encoded[..7], &[10, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&encoded[7..], b"test_short");
    assert_eq!(writer.sequence(), 1);

    let mut reader = CompressedReader::new(Cursor::new(encoded), COMPRESSION_ZLIB, 0);
    let mut decoded = Vec::new();
    reader.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, b"test_short");
    assert_eq!(reader.sequence(), 1);
}

/// 长载荷应对 zlib / zstd 均可往返；压缩头第 4–6 字节为未压缩长度。
#[test]
fn compressed_long_payload_round_trips_zlib_and_zstd() {
    for algorithm in [COMPRESSION_ZLIB, COMPRESSION_ZSTD] {
        let payload = b"test codec test codec test codec test codec test codec test codec";
        let mut writer = CompressedWriter::new(algorithm, 0, 3);
        writer.write_all(payload).unwrap();
        let encoded = writer.flush().unwrap();

        assert_eq!(encoded[3], 0);
        assert_eq!(u24(&encoded[4..7]), payload.len());

        let mut reader = CompressedReader::new(Cursor::new(encoded), algorithm, 0);
        let mut decoded = Vec::new();
        reader.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, payload);
    }
}

/// 外层压缩包序号错误应在 CompressedReader 读时失败。
#[test]
fn compressed_reader_rejects_wrong_outer_sequence() {
    let encoded = vec![1, 0, 0, 1, 0, 0, 0, b'x'];
    let mut reader = CompressedReader::new(Cursor::new(encoded), COMPRESSION_ZLIB, 0);
    let mut byte = [0; 1];
    assert!(
        reader
            .read_exact(&mut byte)
            .unwrap_err()
            .to_string()
            .contains("sequence")
    );
}

/// 开启压缩后忽略内层普通包序号错误，与 Go 及 MariaDB Connector/J 兼容。
#[test]
fn compressed_packet_io_ignores_wrong_inner_sequence_like_go() {
    // Outer packet sequence is 0. Its uncompressed inner packet deliberately
    // uses sequence 1, matching the MariaDB Connector/J compatibility case.
    // 外层序号为 0；解压后内层包故意使用序号 1，对齐 MariaDB Connector/J 兼容场景。
    let encoded = vec![
        14, 0, 0, 0, 0, 0, 0, 10, 0, 0, 1, 3, b's', b'e', b'l', b'e', b'c', b't', b' ', b'1', b';',
    ];
    let mut packet = PacketIO::new_packet_io(Box::new(Cursor::new(encoded)), 1024);
    packet.set_compression_algorithm(COMPRESSION_ZLIB).unwrap();

    assert_eq!(packet.read_packet().unwrap(), b"\x03select 1;");
    assert_eq!(packet.sequence(), 1);
    assert_eq!(packet.compressed_sequence(), 1);
}

/// 解析小端 3 字节无符号整数（MySQL 协议长度字段）。
fn u24(bytes: &[u8]) -> usize {
    bytes[0] as usize | (bytes[1] as usize) << 8 | (bytes[2] as usize) << 16
}
