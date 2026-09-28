// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `key_adapter` 模块单元测试：Noop/DupDetect 编解码、排序、预分配缓冲与 MinRowID。

use crate::{DupDetectKeyAdapter, EncodeIntRowID, KeyAdapter, MinRowID, NoopKeyAdapter};
use std::cmp::Ordering;
use std::io::Read;

/// 生成 `n` 字节伪随机数据；无 `/dev/urandom` 时用确定性公式回退。
fn rand_bytes(n: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; n];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let _ = file.read_exact(&mut bytes);
    } else {
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(31).wrapping_add(17);
        }
    }
    bytes
}

/// 判断两切片是否共享同一起始指针（用于验证缓冲是否原地扩容/重分配）。
fn start_with_same_memory(x: &[u8], y: &[u8]) -> bool {
    !x.is_empty() && !y.is_empty() && std::ptr::eq(x.as_ptr(), y.as_ptr())
}

/// Noop：长度、编码结果、解码结果均等于原始键。
#[test]
fn test_noop_key_adapter() {
    let key_adapter = NoopKeyAdapter;
    let key = rand_bytes(32);
    let row_id = rand_bytes(8);
    assert_eq!(key.len(), key_adapter.EncodedLen(&key, &row_id));
    let encoded_key = key_adapter.Encode(Vec::new(), &key, &row_id);
    assert_eq!(key, encoded_key);
    let decoded_key = key_adapter.Decode(Vec::new(), &encoded_key).unwrap();
    assert_eq!(key, decoded_key);
}

/// DupDetect：多种 RowID 下 EncodedLen 与 Decode 往返正确。
#[test]
fn test_dup_detect_key_adapter() {
    let inputs = [
        (vec![0x0], 0_i64),
        (rand_bytes(32), 1_i64),
        (rand_bytes(32), i32::MAX as i64),
        (rand_bytes(32), i32::MIN as i64),
    ];
    let key_adapter = DupDetectKeyAdapter;
    for (key, row_id) in inputs {
        let encoded_row_id = EncodeIntRowID(row_id);
        let result = key_adapter.Encode(Vec::new(), &key, &encoded_row_id);
        assert_eq!(key_adapter.EncodedLen(&key, &encoded_row_id), result.len());
        assert_eq!(key, key_adapter.Decode(Vec::new(), &result).unwrap());
    }
}

/// DupDetect must reject malformed memcomparable padding like codec.DecodeBytes.
#[test]
fn test_dup_detect_decode_rejects_invalid_padding() {
    let key_adapter = DupDetectKeyAdapter;
    let mut data = key_adapter.Encode(Vec::new(), b"a", &[]);
    data[1] = 1;

    let error = key_adapter.Decode(Vec::new(), &data).unwrap_err();
    assert!(error.to_string().contains("invalid padding byte"));

    let mut data = key_adapter.Encode(Vec::new(), b"", &[]);
    data[8] = 0xf6;
    let error = key_adapter.Decode(Vec::new(), &data).unwrap_err();
    assert!(error.to_string().contains("invalid marker byte"));

    let error = key_adapter.Decode(Vec::new(), &[1, 2, 0, 0]).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("insufficient bytes to decode value")
    );
}

/// 业务键字典序应在 DupDetect 编码后保持。
#[test]
fn test_dup_detect_key_order() {
    let keys = [
        vec![0x0, 0x1, 0x2],
        vec![0x0, 0x1, 0x3],
        vec![0x0, 0x1, 0x3, 0x4],
        vec![0x0, 0x1, 0x3, 0x4, 0x0],
        vec![0x0, 0x1, 0x3, 0x4, 0x0, 0x0, 0x0],
    ];
    let key_adapter = DupDetectKeyAdapter;
    let encoded_keys = keys
        .iter()
        .map(|key| key_adapter.Encode(Vec::new(), key, &EncodeIntRowID(1)))
        .collect::<Vec<_>>();
    assert!(encoded_keys.windows(2).all(|pair| pair[0] < pair[1]));
}

/// 相同业务键、不同 RowID 时编码结果必须不同，才能区分重复项。
#[test]
fn test_dup_detect_encode_dup_key() {
    let key_adapter = DupDetectKeyAdapter;
    let key = rand_bytes(32);
    let result1 = key_adapter.Encode(Vec::new(), &key, &EncodeIntRowID(10));
    let result2 = key_adapter.Encode(Vec::new(), &key, &EncodeIntRowID(20));
    assert_ne!(result1, result2);
}

/// Encode 写入预分配缓冲时应尽量复用同一底层指针（前缀已占用）。
#[test]
fn test_encode_key_to_pre_allocated_buf() {
    for key_adapter in adapters() {
        let key = rand_bytes(32);
        let mut buf = vec![0_u8; 256];
        buf.truncate(4);
        let original_ptr = buf.as_ptr();
        let buf2 = key_adapter.Encode(buf, &key, &EncodeIntRowID(1));
        assert!(std::ptr::eq(original_ptr, buf2.as_ptr()));
        let key2 = key_adapter.Decode(Vec::new(), &buf2[4..]).unwrap();
        assert_eq!(key, key2);
    }
}

/// Decode 写入容量足够的预分配缓冲时应原地扩展。
#[test]
fn test_decode_key_to_pre_allocated_buf() {
    let data = dup_detect_sample_data();
    for key_adapter in adapters() {
        let key = key_adapter.Decode(Vec::new(), &data).unwrap();
        let mut buf = vec![0_u8; 4 + data.len()];
        buf.truncate(4);
        let original_ptr = buf.as_ptr();
        let buf2 = key_adapter.Decode(buf, &data).unwrap();
        assert!(std::ptr::eq(original_ptr, buf2.as_ptr()));
        assert_eq!(key, buf2[4..].to_vec());
    }
}

/// dst 容量不足时允许重分配，但应保留原前缀内容。
#[test]
fn test_decode_key_dst_is_insufficient() {
    let data = dup_detect_sample_data();
    for key_adapter in adapters() {
        let key = key_adapter.Decode(Vec::new(), &data).unwrap();
        let mut buf = Vec::with_capacity(6);
        buf.extend_from_slice(b"abcd");
        let buf2 = key_adapter.Decode(buf.clone(), &data).unwrap();
        assert!(!start_with_same_memory(&buf, &buf2));
        assert_eq!(&buf[..], &buf2[..4]);
        assert_eq!(key, buf2[4..].to_vec());
    }
}

/// MinRowID 编码结果应不大于各类 IntHandle / common-handle 风格 RowID。
#[test]
fn test_min_row_id() {
    let key_adapter = DupDetectKeyAdapter;
    let should_be_min = key_adapter.Encode(b"key".to_vec(), b"val", &MinRowID);

    let mut row_ids = Vec::new();
    // DDL IntHandle-style and Lightning comparable-varint style row IDs.
    for id in [i64::MIN, -1, 0, i64::MAX] {
        row_ids.push(EncodeIntRowID(id));
    }
    // Common-handle-like payloads: non-zero leading markers so they stay above MinRowID,
    // matching the relative order of TiDB codec.EncodeKey outputs used in the Go test.
    for payload in [
        vec![0x01],
        vec![0x01, 0, 0, 0, 0, 0, 0, 0],
        vec![0x03, 0, 0, 0, 0, 0, 0, 0, 0],
        {
            let mut bytes = vec![0x03];
            bytes.extend(std::iter::repeat_n(0, 99));
            bytes
        },
    ] {
        row_ids.push(payload);
    }

    for id in row_ids {
        let bs = key_adapter.Encode(b"key".to_vec(), b"val", &id);
        assert_ne!(bs.cmp(&should_be_min), Ordering::Less, "row_id={id:?}");
    }
}

/// 同时覆盖 Noop 与 DupDetect 两种适配器。
fn adapters() -> Vec<Box<dyn KeyAdapter>> {
    vec![Box::new(NoopKeyAdapter), Box::new(DupDetectKeyAdapter)]
}

/// DupDetect 编码样例字节（含 memcomparable 组与尾部 RowID/长度）。
fn dup_detect_sample_data() -> Vec<u8> {
    vec![
        0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8, 0xff, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0xf7,
        0x0, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8, 0x9, 0xa, 0xb, 0xc, 0xd, 0xe, 0xf, 0x0, 0x8,
    ]
}
