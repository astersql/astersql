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

// rowcodec 公共编解码辅助的单元测试。
//
// 验证整数最短定长编解码、列排序与 NULL 保留、large 行 checksum、
// DatumMap 往返，以及 FieldType flag / row key / 新格式判定与 Go 一致。

use super::*;

/// 有符号/无符号整数按 Go 最短宽度编码并往返解码。
#[test]
fn integer_storage_uses_go_widths_and_round_trips() {
    for (value, width) in [
        (-129_i64, 2_usize),
        (-128, 1),
        (127, 1),
        (128, 2),
        (i16::MAX as i64, 2),
        (i16::MAX as i64 + 1, 4),
        (i32::MAX as i64 + 1, 8),
    ] {
        let encoded = encodeInt(Vec::new(), value);
        assert_eq!(encoded.len(), width);
        assert_eq!(decodeInt(&encoded), value);
    }

    for (value, width) in [
        (u8::MAX as u64, 1_usize),
        (u8::MAX as u64 + 1, 2),
        (u16::MAX as u64 + 1, 4),
        (u32::MAX as u64 + 1, 8),
    ] {
        let encoded = encodeUint(Vec::new(), value);
        assert_eq!(encoded.len(), width);
        assert_eq!(decodeUint(&encoded), value);
    }
}

/// 编码器按列 ID 排序，并正确保留 NULL 列。
#[test]
fn encoder_sorts_columns_and_preserves_nulls() {
    let mut int_value = types::Datum::default();
    int_value.SetInt64(-42);
    let mut bytes_value = types::Datum::default();
    bytes_value.SetBytes(b"tidb".to_vec());
    let null_value = types::Datum::default();

    let mut encoder = Encoder::new(true);
    let encoded = encoder
        .Encode(
            None,
            vec![9, 2, 7],
            vec![bytes_value, int_value, null_value],
            None,
            Vec::new(),
        )
        .expect("encode row");

    let mut decoded = row::default();
    decoded.fromBytes(&encoded).expect("decode row header");
    assert_eq!(decoded.colIDs, vec![2, 9, 7]);
    assert_eq!(decoded.findColID(2), (0, false, false));
    assert_eq!(decodeInt(decoded.getData(0)), -42);
    assert_eq!(decoded.findColID(7), (0, true, false));
    assert_eq!(decoded.getData(1), b"tidb");
}

/// large 行（列 ID ≥ 256）与 raw handle checksum 布局与 Go 一致。
#[test]
fn large_row_and_checksum_follow_go_layout() {
    let mut value = types::Datum::default();
    value.SetUint64(300);
    let mut encoder = Encoder::new(true);
    let handle: Box<dyn kv::Handle> = Box::new(kv::IntHandle(11));
    let encoded = encoder
        .Encode(
            None,
            vec![300],
            vec![value],
            Some(Box::new(RawChecksum { Handle: handle })),
            Vec::new(),
        )
        .expect("encode checksummed row");

    let mut decoded = row::default();
    decoded.fromBytes(&encoded).expect("decode checksummed row");
    assert!(decoded.large());
    assert_eq!(decoded.colIDs32, vec![300]);
    assert_eq!(decoded.ChecksumVersion(), checksumVersionRawHandle as i32);
    let (stored_checksum, ok) = decoded.GetChecksum();
    assert!(ok);
    let checksum_offset = encoded.len() - 4;
    let checksum_from_bytes = u32::from_le_bytes(
        encoded[checksum_offset..]
            .try_into()
            .expect("checksum bytes"),
    );
    assert_eq!(stored_checksum, checksum_from_bytes);
    let handle_bytes = kv::Handle::Encoded(&kv::IntHandle(11));
    let expected = crc32_update(crc32_update(0, &encoded[..checksum_offset]), &handle_bytes);
    assert_eq!(stored_checksum, expected);
}

/// DatumMapDecoder 对整数与字节列往返正确。
#[test]
fn datum_map_decoder_round_trips_integer_and_bytes() {
    let mut integer = types::Datum::default();
    integer.SetInt64(-1234);
    let mut bytes = types::Datum::default();
    bytes.SetBytes(b"rowcodec".to_vec());

    let encoded = Encoder::new(true)
        .Encode(None, vec![4, 1], vec![bytes, integer], None, Vec::new())
        .expect("encode row");

    let mut int_type = types::FieldType::default();
    int_type.SetType(mysql::TypeLonglong);
    let mut bytes_type = types::FieldType::default();
    bytes_type.SetType(mysql::TypeBlob);
    let columns = vec![
        ColInfo {
            ID: 1,
            IsPKHandle: false,
            VirtualGenCol: false,
            Ft: int_type,
        },
        ColInfo {
            ID: 4,
            IsPKHandle: false,
            VirtualGenCol: false,
            Ft: bytes_type,
        },
    ];
    let decoded = NewDatumMapDecoder(columns, None)
        .DecodeToDatumMap(&encoded, None)
        .expect("decode datum map");
    assert_eq!(decoded[&1].GetInt64(), -1234);
    assert_eq!(decoded[&4].GetBytes(), b"rowcodec");
}

/// FieldType 到旧 Datum 编码 flag 的映射与 Go 一致。
#[test]
fn field_type_flags_match_old_datum_encoding() {
    assert_eq!(fieldType2Flag(mysql::TypeLonglong, true), IntFlag);
    assert_eq!(fieldType2Flag(mysql::TypeLonglong, false), UintFlag);
    assert_eq!(fieldType2Flag(mysql::TypeVarchar, true), BytesFlag);
    assert_eq!(fieldType2Flag(mysql::TypeJSON, true), JSONFlag);
    assert_eq!(fieldType2Flag(mysql::TypeNull, true), NilFlag);
}

/// IsRowKey / IsNewFormat 判定与 Go 行为一致。
#[test]
fn row_key_and_new_format_checks_match_go() {
    let mut key = vec![0_u8; rowKeyLen];
    key[0] = b't';
    key[recordPrefixIdx] = b'r';
    assert!(IsRowKey(&key));
    assert!(!IsRowKey(&key[..rowKeyLen - 1]));
    assert!(IsNewFormat(&[CodecVer]));
    assert!(!IsNewFormat(&[CodecVer - 1]));
}
