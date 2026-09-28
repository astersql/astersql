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

// JSON 分块编解码的边界与损坏载荷单元测试。
//
// 覆盖 gzip 成员尾部多余字节、校验和破坏，以及 JSON 载荷被篡改后的拒绝路径。

use super::{JsonTable, blocks_to_json_table, json_table_to_blocks};

/// 计算 CRC32（IEEE），用于构造合法 gzip trailer。
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

/// 构造单块未压缩 DEFLATE 的 gzip 成员（测试用）。
fn gzip_store(data: &[u8]) -> Vec<u8> {
    assert!(data.len() <= u16::MAX as usize);
    let mut result = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255, 1];
    let length = data.len() as u16;
    result.extend(length.to_le_bytes());
    result.extend((!length).to_le_bytes());
    result.extend(data);
    result.extend(crc32(data).to_le_bytes());
    result.extend((data.len() as u32).to_le_bytes());
    result
}

/// 合法块在附加尾字节、破坏 isize、或篡改 JSON 后均应解码失败。
#[test]
fn gzip_and_binary_payload_must_be_fully_consumed() {
    let blocks = json_table_to_blocks(&JsonTable::default(), usize::MAX).unwrap();
    let canonical = blocks.concat();

    // 尾部多余字节：gzip member 必须被完整消费。
    let mut trailing_member = canonical.clone();
    trailing_member.push(0);
    assert!(blocks_to_json_table(&[trailing_member]).is_err());

    // 翻转 isize 末字节触发 size mismatch。
    let mut bad_size = canonical.clone();
    let last = bad_size.len() - 1;
    bad_size[last] ^= 1;
    assert!(blocks_to_json_table(&[bad_size]).is_err());

    // 破坏 JSON 闭合，使二进制载荷无法解析为合法 JsonTable。
    let payload_length = u16::from_le_bytes([canonical[11], canonical[12]]) as usize;
    let json = &canonical[15..15 + payload_length];
    let mut json_with_binary_trailing = json[..json.len() - 2].to_vec();
    json_with_binary_trailing.extend_from_slice(b"00\"}");
    let malformed = gzip_store(&json_with_binary_trailing);
    assert!(blocks_to_json_table(&[malformed]).is_err());
}

#[test]
fn historical_payload_is_canonical_across_hash_map_insertion_order() {
    let first = JsonTable {
        stats: crate::TableStats {
            columns: [
                ("b".to_owned(), Default::default()),
                ("a".to_owned(), Default::default()),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        },
        ..Default::default()
    };
    let second = JsonTable {
        stats: crate::TableStats {
            columns: [
                ("a".to_owned(), Default::default()),
                ("b".to_owned(), Default::default()),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(
        json_table_to_blocks(&first, 1024).unwrap(),
        json_table_to_blocks(&second, 1024).unwrap()
    );
}
