// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// Aster 迁移补充单测：行/索引/元数据 key 编解码与临时索引、范围校验。
//
// 覆盖整数 handle、common handle（联合主键列编码）、meta key，
// 以及临时索引前缀转换与多表 KeyRange 的 table id 一致性检查。

use super::*;
use codec::EncodeInt;
use kv::{IntHandle, KeyRange, NewCommonHandle, NewNonPartitionedKeyRanges};

/// 验证整数 handle 与 common handle 的行 key 往返编解码。
#[test]
fn row_key_round_trips_int_and_common_handles() {
    let encoded = EncodeInt(Vec::new(), 2);
    let key = EncodeRowKey(1, &encoded);
    let handle = DecodeRowKey(key).expect("integer record key must decode");
    assert!(handle.IsInt());
    assert_eq!(handle.IntValue(), 2);

    let key = EncodeRowKeyWithHandle(7, Box::new(IntHandle(-19)));
    let (table_id, handle) = DecodeRecordKey(key).expect("record key must decode");
    assert_eq!(table_id, 7);
    assert_eq!(handle.IntValue(), -19);

    let encoded_common = codec::EncodeKey(
        chrono_tz::UTC,
        Vec::new(),
        vec![types::NewIntDatum(3), types::NewBytesDatum(b"abc".to_vec())],
    )
    .expect("common handle columns must encode");
    let common = NewCommonHandle(encoded_common).expect("common handle must be valid");
    let key = EncodeRowKeyWithHandle(9, Box::new(common));
    let (table_id, decoded) = DecodeRecordKey(key).expect("common record key must decode");
    assert_eq!(table_id, 9);
    assert!(!decoded.IsInt());
    assert_eq!(decoded.NumCols(), 2);
}

/// 验证记录/索引前缀解码与 meta key 布局与 Go 一致。
#[test]
fn table_index_and_meta_prefixes_match_go_layout() {
    let record = EncodeRowKeyWithHandle(42, Box::new(IntHandle(7)));
    let (table_id, index_id, is_record) =
        DecodeKeyHead(record.clone()).expect("record prefix must decode");
    assert_eq!((table_id, index_id, is_record), (42, 0, true));
    assert!(IsRecordKey(record.as_ref()));

    let index = EncodeIndexSeekKey(42, 5, Some(vec![codec::NilFlag]));
    let (table_id, index_id, is_record) =
        DecodeKeyHead(index.clone()).expect("index key must decode");
    assert_eq!((table_id, index_id, is_record), (42, 5, false));
    assert!(IsIndexKey(index.as_ref()));
    assert_eq!(DecodeIndexID(index).unwrap(), 5);

    let encoded = EncodeMetaKey(b"DB:1", b"TID:2");
    let (key, field) = DecodeMetaKey(encoded).expect("meta key must decode");
    assert_eq!(key, b"DB:1");
    assert_eq!(field, b"TID:2");
}

/// 验证临时索引 key 互转，以及 KeyRange 必须属于同一 table id。
#[test]
fn temporary_index_and_range_validation_preserve_go_boundaries() {
    let mut index = EncodeIndexSeekKey(88, 11, None);
    assert!(!IsTempIndexKey(index.as_ref()));
    IndexKey2TempIndexKey(index.0.as_mut_slice());
    assert!(IsTempIndexKey(index.as_ref()));
    TempIndexKey2IndexKey(index.0.as_mut_slice());
    assert!(!IsTempIndexKey(index.as_ref()));

    let ranges = NewNonPartitionedKeyRanges(vec![KeyRange {
        StartKey: GenTableRecordPrefix(88),
        EndKey: GenTableIndexPrefix(88),
    }]);
    assert_eq!(VerifyTableIDForRanges(Box::new(ranges)).unwrap(), vec![88]);

    let mixed = NewNonPartitionedKeyRanges(vec![
        KeyRange {
            StartKey: GenTableRecordPrefix(88),
            EndKey: GenTableIndexPrefix(88),
        },
        KeyRange {
            StartKey: GenTableRecordPrefix(89),
            EndKey: GenTableIndexPrefix(89),
        },
    ]);
    assert!(VerifyTableIDForRanges(Box::new(mixed)).is_err());
}

/// tablecodec 的公开契约必须直接使用已校验的 canonical model/kv 类型。
#[test]
fn public_contract_uses_canonical_model_and_kv_types() {
    fn accept_canonical_table(_: model_group_1::TableInfo) {}

    accept_canonical_table(model::TableInfo::default());

    let key: structure_dependency::kv::Key =
        EncodeRowKeyWithHandle(91, Box::new(structure_dependency::kv::IntHandle(7)));
    assert_eq!(DecodeTableID(key), 91);

    let ranges = structure_dependency::kv::NewNonPartitionedKeyRanges(vec![
        structure_dependency::kv::KeyRange {
            StartKey: GenTableRecordPrefix(91),
            EndKey: GenTableIndexPrefix(91),
        },
    ]);
    assert_eq!(VerifyTableIDForRanges(Box::new(ranges)).unwrap(), vec![91]);
}
