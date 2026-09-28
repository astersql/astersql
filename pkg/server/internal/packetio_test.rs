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

// PacketIO 单元测试：写包、压缩写/读与分包拼接。
//
// 对照 Go `packetio_test.go`，覆盖普通包头、MaxPayloadLen 分包、
// zlib/zstd 压缩帧以及内层错误序号在压缩模式下的兼容行为。

use std::io::{Cursor, Read, Write};

use super::*;

const STRESS_CASES: usize = 10_000;
const STRESS_SEED: u64 = 0x4a17_2026_0824_d00d;

struct FragmentedReader {
    bytes: Cursor<Vec<u8>>,
    max_chunk: usize,
}

impl FragmentedReader {
    fn new(bytes: Vec<u8>, max_chunk: usize) -> Self {
        Self {
            bytes: Cursor::new(bytes),
            max_chunk: max_chunk.max(1),
        }
    }
}

impl Read for FragmentedReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let limit = output.len().min(self.max_chunk);
        self.bytes.read(&mut output[..limit])
    }
}

fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn packet_bytes(sequence: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![
        payload.len() as u8,
        (payload.len() >> 8) as u8,
        (payload.len() >> 16) as u8,
        sequence,
    ];
    bytes.extend_from_slice(payload);
    bytes
}

fn assert_stress_case(case: usize, seed: u64, bytes: &[u8], result: Result<Vec<u8>, String>) {
    assert!(
        result.is_err(),
        "case={case} seed=0x{seed:016x} bytes={bytes:02x?} unexpectedly succeeded"
    );
}

#[test]
fn deterministic_fragmented_and_malformed_packet_corpus_is_bounded() {
    let mut state = STRESS_SEED;
    for case in 0..STRESS_CASES {
        let seed = next_random(&mut state);
        let chunk = (seed as usize % 7) + 1;
        let payload_len = ((seed >> 8) as usize % 96) + 1;
        let payload: Vec<u8> = (0..payload_len)
            .map(|index| seed.rotate_left(index as u32) as u8)
            .collect();

        match case % 10 {
            0 | 1 => {
                let bytes = packet_bytes(0, &payload);
                let mut packet =
                    new_packet_io(Box::new(FragmentedReader::new(bytes.clone(), chunk)), 1024);
                assert_eq!(
                    packet.read_packet().unwrap_or_else(|error| panic!(
                        "case={case} seed=0x{seed:016x} bytes={bytes:02x?}: {error}"
                    )),
                    payload
                );
            }
            2 => {
                let bytes = packet_bytes(0, &payload)[..(seed as usize % 4)].to_vec();
                let mut packet =
                    new_packet_io(Box::new(FragmentedReader::new(bytes.clone(), chunk)), 1024);
                let result = packet.read_packet();
                assert_stress_case(case, seed, &bytes, result);
            }
            3 => {
                let mut bytes = packet_bytes(0, &payload);
                bytes.truncate(4 + payload_len / 2);
                let mut packet =
                    new_packet_io(Box::new(FragmentedReader::new(bytes.clone(), chunk)), 1024);
                let result = packet.read_packet();
                assert_stress_case(case, seed, &bytes, result);
            }
            4 => {
                let wrong_sequence = 1_u8.wrapping_add(seed as u8).max(1);
                let bytes = packet_bytes(wrong_sequence, &payload);
                let mut packet =
                    new_packet_io(Box::new(FragmentedReader::new(bytes.clone(), chunk)), 1024);
                let result = packet.read_packet();
                assert_stress_case(case, seed, &bytes, result);
            }
            5 => {
                let bytes = vec![0xff, 0xff, 0xff, 0];
                let mut packet =
                    new_packet_io(Box::new(FragmentedReader::new(bytes.clone(), chunk)), 1024);
                let result = packet.read_packet();
                assert_stress_case(case, seed, &bytes, result);
            }
            6 => {
                let bytes = vec![4, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0];
                let mut packet =
                    new_packet_io(Box::new(FragmentedReader::new(bytes.clone(), chunk)), 1024);
                packet.set_compression_algorithm(COMPRESSION_ZLIB).unwrap();
                let result = packet.read_packet();
                assert_stress_case(case, seed, &bytes, result);
            }
            7 => {
                let bytes = vec![0, 0, 0, 1, 0, 0, 0];
                let mut packet =
                    new_packet_io(Box::new(FragmentedReader::new(bytes.clone(), chunk)), 1024);
                packet.set_compression_algorithm(COMPRESSION_ZLIB).unwrap();
                let result = packet.read_packet();
                assert_stress_case(case, seed, &bytes, result);
            }
            8 => {
                let inner = packet_bytes(7, &payload);
                let mut bytes = vec![
                    inner.len() as u8,
                    (inner.len() >> 8) as u8,
                    (inner.len() >> 16) as u8,
                    0,
                    0,
                    0,
                    0,
                ];
                bytes.extend_from_slice(&inner);
                let mut packet =
                    new_packet_io(Box::new(FragmentedReader::new(bytes.clone(), chunk)), 1024);
                packet.set_compression_algorithm(COMPRESSION_ZLIB).unwrap();
                assert_eq!(
                    packet.read_packet().unwrap_or_else(|error| panic!(
                        "case={case} seed=0x{seed:016x} bytes={bytes:02x?}: {error}"
                    )),
                    payload
                );
            }
            9 => {
                let bytes = vec![8, 0, 0, 0, 0, 0, 0, 1, 2];
                let mut packet =
                    new_packet_io(Box::new(FragmentedReader::new(bytes.clone(), chunk)), 1024);
                packet.set_compression_algorithm(COMPRESSION_ZLIB).unwrap();
                let result = packet.read_packet();
                assert_stress_case(case, seed, &bytes, result);
            }
            _ => unreachable!(),
        }
    }
    eprintln!("packetio deterministic corpus cases={STRESS_CASES} seed=0x{STRESS_SEED:016x}");
}

#[test]
fn zlib_decompression_stops_at_the_advertised_output_boundary() {
    let expanded = vec![b'A'; 64 * 1024];
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&expanded).unwrap();
    let compressed = encoder.finish().unwrap();

    let result = decompress(&compressed, COMPRESSION_ZLIB, 8);
    assert!(
        matches!(
            result,
            Err(ref error) if error.kind() == std::io::ErrorKind::InvalidData
        ),
        "unexpected bounded zlib result: {:?}",
        result.as_ref().map(Vec::len)
    );
}

/// Rust counterpart of Go's `BenchmarkPacketIOWrite`. The harness can choose
/// the iteration count without requiring an unstable benchmark framework.
/// 对应 Go `BenchmarkPacketIOWrite`：循环写固定样例包，由外部指定迭代次数。
pub fn benchmark_packet_io_write(iterations: usize) {
    for _ in 0..iterations {
        let mut packet = new_packet_io_for_test(Vec::new());
        let mut data = vec![
            0x6d, 0x44, 0x42, 0x3a, 0x35, 0x36, 0x00, 0x00, 0x00, 0xfc, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x68, 0x54, 0x49, 0x44, 0x3a, 0x31, 0x30, 0x38, 0x00, 0xfe,
        ];
        let _ = packet.write_packet(&mut data);
    }
}

/// 校验小包头重写，以及恰好超过 MAX_PAYLOAD_LEN 时的满长分包头。
#[test]
fn test_packet_io_write() {
    let mut packet = new_packet_io_for_test(Vec::new());
    let mut data = vec![0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03];
    packet.write_packet(&mut data).unwrap();
    packet.flush().unwrap();
    assert_eq!(
        packet.written_data(),
        &[0x03, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03]
    );

    let mut packet = new_packet_io_for_test(Vec::new());
    let mut large_input = vec![0_u8; MAX_PAYLOAD_LEN + 4];
    packet.write_packet(&mut large_input).unwrap();
    packet.flush().unwrap();
    // 第一分包头长度字段应为 0xFFFFFF。
    assert_eq!(&packet.written_data()[..4], &[0xff, 0xff, 0xff, 0x00]);
}

/// Production transports consume only bytes that have been fully encoded by
/// PacketIO, and later writes must remain usable after that consumption.
#[test]
fn encoded_output_can_be_safely_taken_between_socket_flushes() {
    let mut packet = new_packet_io_for_test(Vec::new());
    let mut first = vec![0, 0, 0, 0, b'o', b'n', b'e'];
    packet.write_packet(&mut first).unwrap();
    packet.flush().unwrap();

    assert_eq!(
        packet.take_written_data(),
        vec![3, 0, 0, 0, b'o', b'n', b'e']
    );
    assert!(packet.written_data().is_empty());

    let mut second = vec![0, 0, 0, 0, b't', b'w', b'o'];
    packet.write_packet(&mut second).unwrap();
    packet.flush().unwrap();
    assert_eq!(
        packet.take_written_data(),
        vec![3, 0, 0, 1, b't', b'w', b'o']
    );
}

/// 大载荷经 zlib 压缩写出后，压缩头未压缩长度应为 1MiB（首个满缓冲块）。
#[test]
fn test_packet_io_write_compressed() {
    let mut packet = new_packet_io_for_test(Vec::new());
    packet.set_compression_algorithm(COMPRESSION_ZLIB).unwrap();
    let mut payload = vec![b'A'; 16 * 1024 * 1024 + 4];
    packet.write_packet(&mut payload).unwrap();
    packet.flush().unwrap();

    let data = packet.written_data();
    let compressed_length = header_payload_length(&data[..3]);
    assert!(compressed_length > 0);
    assert!(data.len() >= 7 + compressed_length);
    assert_eq!(&data[3..4], &[0x00]);
    assert_eq!(&data[4..7], &[0x00, 0x00, 0x10]);
    let mut decoded = Vec::new();
    flate2::read::ZlibDecoder::new(&data[7..7 + compressed_length])
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded.len(), 1024 * 1024);
    assert_eq!(&decoded[..4], &[0xff, 0xff, 0xff, 0x00]);
    assert!(decoded[4..].iter().all(|byte| *byte == b'A'));
}

/// 覆盖未压缩单包/分包读，以及压缩短包与 zlib/zstd 长注释查询载荷。
#[test]
fn test_packet_io_read() {
    // uncompressed: one packet, then a payload split at MaxPayloadLen.
    // 未压缩：先读单包，再读在 MaxPayloadLen 处分包的载荷。
    let input = vec![0x01, 0x00, 0x00, 0x00, 0x01];
    let mut packet = new_packet_io(Box::new(Cursor::new(input)), u64::MAX);
    assert_eq!(packet.read_packet().unwrap(), vec![0x01]);
    assert_eq!(packet.sequence(), 1);

    let mut input = vec![0_u8; MAX_PAYLOAD_LEN + 9];
    input[..4].copy_from_slice(&[0xff, 0xff, 0xff, 0x00]);
    input[MAX_PAYLOAD_LEN + 4..MAX_PAYLOAD_LEN + 8].copy_from_slice(&[0x01, 0x00, 0x00, 0x01]);
    input[MAX_PAYLOAD_LEN + 8] = 0x0a;
    let mut packet = new_packet_io(Box::new(Cursor::new(input)), u64::MAX);
    let read = packet.read_packet().unwrap();
    assert_eq!(packet.sequence(), 2);
    assert_eq!(read.len(), MAX_PAYLOAD_LEN + 1);
    assert_eq!(read[MAX_PAYLOAD_LEN], 0x0a);

    // A compressed header with zero uncompressed length carries plain bytes.
    // 压缩头未压缩长度为 0：载荷为明文普通包字节。
    let input = vec![
        0x27, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x23, 0x00, 0x00, 0x00, 0x03, 0x00, 0x01, 0x73,
        0x65, 0x6c, 0x65, 0x63, 0x74, 0x20, 0x40, 0x40, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e,
        0x5f, 0x63, 0x6f, 0x6d, 0x6d, 0x65, 0x6e, 0x74, 0x20, 0x6c, 0x69, 0x6d, 0x69, 0x74, 0x20,
        0x31,
    ];
    let mut packet = new_packet_io(Box::new(Cursor::new(input)), u64::MAX);
    packet.set_compression_algorithm(COMPRESSION_ZLIB).unwrap();
    assert_eq!(
        packet.read_packet().unwrap(),
        compressed_packet_short_query()
    );
    assert_eq!(packet.sequence(), 1);

    let zlib_input = vec![
        0x39, 0x00, 0x00, 0x00, 0x49, 0x00, 0x00, 0x78, 0x5e, 0x73, 0x65, 0x60, 0x60, 0x60, 0x0e,
        0x76, 0xf5, 0x71, 0x75, 0x0e, 0x51, 0x30, 0x54, 0xd0, 0xd7, 0x52, 0x48, 0x4c, 0x4a, 0x4e,
        0x49, 0x4d, 0x4b, 0xcf, 0xc8, 0xcc, 0xca, 0xce, 0xc9, 0xcd, 0xcb, 0x2f, 0x28, 0x2c, 0x2a,
        0x2e, 0x29, 0x2d, 0x2b, 0xaf, 0xa8, 0xac, 0x8a, 0xc7, 0x2d, 0xa5, 0xa0, 0xa5, 0x0f, 0x00,
        0x59, 0xd8, 0x1a, 0x09,
    ];
    assert_compressed_packet(zlib_input, COMPRESSION_ZLIB, select_comment_payload());

    let zstd_input = vec![
        0x40, 0x00, 0x00, 0x00, 0x49, 0x00, 0x00, 0x28, 0xb5, 0x2f, 0xfd, 0x20, 0x49, 0xbd, 0x01,
        0x00, 0xf4, 0x02, 0x45, 0x00, 0x00, 0x00, 0x03, 0x53, 0x45, 0x4c, 0x45, 0x43, 0x54, 0x20,
        0x31, 0x20, 0x2f, 0x2a, 0x20, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a,
        0x6b, 0x6c, 0x6d, 0x6e, 0x6f, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79,
        0x7a, 0x5f, 0x20, 0x2a, 0x2f, 0x01, 0x00, 0x74, 0x7b, 0x96, 0x01,
    ];
    assert_compressed_packet(zstd_input, COMPRESSION_ZSTD, select_comment_payload());
}

/// 短字符串写入压缩端：头中未压缩长度为 0，载荷原样。
#[test]
fn test_compressed_writer_short() {
    let payload = b"test_short";
    let mut writer = CompressedWriter::new(COMPRESSION_ZLIB, 0, 3);
    writer.write_all(payload).unwrap();
    let data = writer.flush().unwrap();
    assert_eq!(&data[..3], &[0x0a, 0x00, 0x00]);
    assert_eq!(&data[3..4], &[0x00]);
    assert_eq!(&data[4..7], &[0x00, 0x00, 0x00]);
    assert_eq!(&data[7..], payload);
}

/// 长字符串分别经 zlib / zstd 写出后可正确解压还原。
#[test]
fn test_compressed_writer_long() {
    let zlib_payload = b"test_zlib test_zlib test_zlib test_zlib test_zlib test_zlib test_zlib";
    let mut writer = CompressedWriter::new(COMPRESSION_ZLIB, 0, 3);
    writer.write_all(zlib_payload).unwrap();
    let data = writer.flush().unwrap();
    assert_eq!(header_payload_length(&data[..3]), data.len() - 7);
    assert_eq!(&data[3..4], &[0x00]);
    assert_eq!(&data[4..7], &[0x45, 0x00, 0x00]);
    let mut decoded = Vec::new();
    flate2::read::ZlibDecoder::new(&data[7..])
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, zlib_payload);

    let zstd_payload = b"test_zstd test_zstd test_zstd test_zstd test_zstd test_zstd test_zstd";
    let mut writer = CompressedWriter::new(COMPRESSION_ZSTD, 0, 3);
    writer.write_all(zstd_payload).unwrap();
    let data = writer.flush().unwrap();
    assert_eq!(header_payload_length(&data[..3]), data.len() - 7);
    assert_eq!(&data[3..4], &[0x00]);
    assert_eq!(&data[4..7], &[0x45, 0x00, 0x00]);
    let decoded = zstd::bulk::decompress(&data[7..], zstd_payload.len()).unwrap();
    assert_eq!(decoded, zstd_payload);
}

/// 短压缩帧（未压缩长度 0）经 CompressedReader 应还原普通包头与查询。
#[test]
fn test_compressed_reader_short() {
    let payload = vec![
        0x25, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x00, 0x00, 0x03, 0x73, 0x65, 0x6c,
        0x65, 0x63, 0x74, 0x20, 0x40, 0x40, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x5f, 0x63,
        0x6f, 0x6d, 0x6d, 0x65, 0x6e, 0x74, 0x20, 0x6c, 0x69, 0x6d, 0x69, 0x74, 0x20, 0x31,
    ];
    let mut reader = CompressedReader::new(Cursor::new(payload), COMPRESSION_ZLIB, 0);
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).unwrap();
    assert_eq!(header, [0x21, 0x00, 0x00, 0x00]);
    assert_eq!(header_payload_length(&header), 33);
    assert_eq!(header[3], 0);
    let mut data = vec![0_u8; 33];
    reader.read_exact(&mut data).unwrap();
    assert_eq!(data, compressed_short_query());
}

/// zlib / zstd 长压缩帧经 CompressedReader 应还原含长字符串的 SELECT。
#[test]
fn test_compressed_reader_long() {
    let zlib_payload = vec![
        0x19, 0x00, 0x00, 0x00, 0x9c, 0x00, 0x00, 0x78, 0x5e, 0x9b, 0xc1, 0xc0, 0xc0, 0xc0, 0x1c,
        0xec, 0xea, 0xe3, 0xea, 0x1c, 0xa2, 0xa0, 0xe4, 0x38, 0xa8, 0x80, 0x12, 0x00, 0xbe, 0xe6,
        0x26, 0xce,
    ];
    assert_long_reader_payload(CompressedReader::new(
        Cursor::new(zlib_payload),
        COMPRESSION_ZLIB,
        0,
    ));

    let zstd_payload = vec![
        0x1f, 0x00, 0x00, 0x00, 0x9c, 0x00, 0x00, 0x28, 0xb5, 0x2f, 0xfd, 0x20, 0x9c, 0xb5, 0x00,
        0x00, 0x78, 0x98, 0x00, 0x00, 0x00, 0x03, 0x53, 0x45, 0x4c, 0x45, 0x43, 0x54, 0x20, 0x22,
        0x41, 0x22, 0x01, 0x00, 0x0a, 0x0a, 0x28, 0x01,
    ];
    assert_long_reader_payload(CompressedReader::new(
        Cursor::new(zstd_payload),
        COMPRESSION_ZSTD,
        0,
    ));
}

/// 压缩模式下内层普通包序号错误仍应成功读包（对齐 Go 兼容行为）。
#[test]
fn test_sub_header_with_wrong_sequence_number() {
    let input = vec![
        0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x01, 0x03, 0x73, 0x65, 0x6c,
        0x65, 0x63, 0x74, 0x20, 0x31, 0x3b,
    ];
    let mut packet = new_packet_io(Box::new(Cursor::new(input)), u64::MAX);
    packet.set_compression_algorithm(COMPRESSION_ZLIB).unwrap();
    let data = packet.read_packet().unwrap();
    assert_eq!(packet.sequence(), 1);
    assert_eq!(packet.compressed_sequence(), 1);
    assert_eq!(data, b"\x03select 1;");
}

/// 用给定算法读压缩输入并断言载荷与序号。
fn assert_compressed_packet(input: Vec<u8>, algorithm: i32, expected: Vec<u8>) {
    let mut packet = new_packet_io(Box::new(Cursor::new(input)), u64::MAX);
    packet.set_compression_algorithm(algorithm).unwrap();
    assert_eq!(packet.read_packet().unwrap(), expected);
    assert_eq!(packet.sequence(), 1);
}

/// 断言长压缩帧解出的普通包头与 SELECT "A"*142 载荷。
fn assert_long_reader_payload<R: Read>(mut reader: CompressedReader<R>) {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).unwrap();
    assert_eq!(header, [0x98, 0x00, 0x00, 0x00]);
    assert_eq!(header_payload_length(&header), 152);
    assert_eq!(header[3], 0);
    let mut data = vec![0_u8; 152];
    reader.read_exact(&mut data).unwrap();
    assert_eq!(data, long_a_select_payload());
}

/// 解析 3 字节小端长度字段。
fn header_payload_length(header: &[u8]) -> usize {
    header[0] as usize | (header[1] as usize) << 8 | (header[2] as usize) << 16
}

/// 短查询 COM_QUERY 载荷：`select @@version_comment limit 1`。
fn compressed_short_query() -> Vec<u8> {
    let mut expected = vec![0x03];
    expected.extend_from_slice(b"select @@version_comment limit 1");
    expected
}

/// 含额外前缀字节的短查询期望载荷。
fn compressed_packet_short_query() -> Vec<u8> {
    let mut expected = vec![0x03, 0x00, 0x01];
    expected.extend_from_slice(b"select @@version_comment limit 1");
    expected
}

/// 带长注释的 SELECT 1 期望载荷。
fn select_comment_payload() -> Vec<u8> {
    let mut expected = vec![0x03];
    expected
        .extend_from_slice(b"SELECT 1 /* abcdefghijklmnopqrstuvwxyz_abcdefghijklmnopqrstuvwxyz */");
    expected
}

/// SELECT 后跟 142 个 'A' 的期望载荷。
fn long_a_select_payload() -> Vec<u8> {
    let mut expected = vec![0x03];
    expected.extend_from_slice(b"SELECT \"");
    expected.extend(std::iter::repeat_n(b'A', 142));
    expected.push(b'"');
    expected
}
