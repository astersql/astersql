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
// `tablecodec` 编解码单元测试与基准：行/索引/临时索引及 key 范围。
//
// 对齐 Go `tablecodec_test.go` 的用例顺序与断言，覆盖新旧行编码、
// handle 编解码、全局索引分区 ID、以及临时索引 value 编解码。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::Mutex;

use super::*;

/// 保护 decimal failpoint 相关用例的互斥锁。
static DECIMAL_FAILPOINT_LOCK: Mutex<()> = Mutex::new(());

/// 构造默认时区的 codec Encoder。
fn defaultCodecEncoder() -> codec::Encoder {
    codec::NewEncoder(collate::NewCollationEnabled())
}

/// 按类型字节构造简单 FieldType。
fn field_type(tp: u8) -> Box<types::FieldType> {
    types::NewFieldType(tp)
}

/// 从十进制字符串解析 MyDecimal。
fn decimal(value: &str) -> types::MyDecimal {
    let mut result = types::MyDecimal::default();
    result.FromString(value.as_bytes()).unwrap();
    result
}

/// 比较两个 Datum 的类型与值是否一致。
fn assert_datum_equal(actual: &types::Datum, expected: &types::Datum) {
    let collator = collate::GetBinaryCollator();
    assert_eq!(
        actual
            .Compare(
                (*types::DefaultStmtNoWarningContext).clone(),
                expected,
                collator.as_ref(),
            )
            .unwrap(),
        0,
        "actual={:?}, expected={:?}",
        actual.GetValue(),
        expected.GetValue()
    );
}

/// 使用旧版行编码路径编码单个 Datum。
fn encode_old_value(value: types::Datum) -> Vec<u8> {
    let encoded = EncodeOldRow(Some(time::UTC), vec![value], vec![1], Vec::new(), None).unwrap();
    let (_, remain) = codec::CutOne(encoded).unwrap();
    let (value, _) = codec::CutOne(remain).unwrap();
    value
}

#[test]
fn go_merge_9_row_encoding_uses_package_codec() {
    let row = vec![types::NewStringDatum("row".to_owned())];
    let old = EncodeOldRow(Some(time::UTC), row.clone(), vec![7], Vec::new(), None).unwrap();
    let expected = codec::EncodeValue(
        time::UTC,
        Vec::new(),
        vec![types::NewIntDatum(7), row[0].clone()],
    )
    .unwrap();
    assert_eq!(old, expected);

    let fallback = EncodeRow(
        Some(time::UTC),
        row,
        vec![7],
        Vec::new(),
        None,
        None,
        rowcodec::Encoder::new(false),
    )
    .unwrap();
    assert_eq!(fallback, old);
}

/// 构造临时索引 value 元素测试夹具。
fn temp_elem(
    value: Vec<u8>,
    handle: Box<dyn kv::Handle>,
    key_ver: u8,
    delete: bool,
    distinct: bool,
) -> TempIndexValueElem {
    TempIndexValueElem {
        Value: value,
        Handle: handle,
        KeyVer: key_ver,
        Delete: delete,
        Distinct: distinct,
        Global: false,
    }
}

#[test]
/// 验证表前缀与基本 key 编解码往返。
fn TestTableCodec() {
    let encoded = codec::EncodeInt(Vec::new(), 2);
    let handle = DecodeRowKey(EncodeRowKey(1, &encoded)).unwrap();
    assert_eq!(handle.IntValue(), 2);

    let handle = DecodeRowKey(EncodeRowKeyWithHandle(1, Box::new(kv::IntHandle(2)))).unwrap();
    assert_eq!(handle.IntValue(), 2);
}

#[test]
/// 验证非法 key 返回对应错误。
fn TestTableCodecInvalid() {
    let mut key = Vec::with_capacity(20);
    key.push(b't');
    key = codec::EncodeInt(key, 100);
    key.extend_from_slice(b"_r");
    key = codec::EncodeInt(key, -9_078_412_423_848_787_968);
    key.push(b'0');
    let error = match DecodeRowKey(kv::Key(key)) {
        Ok(_) => panic!("invalid row key unexpectedly decoded"),
        Err(error) => error,
    };
    assert_eq!(error.to_string(), "invalid encoded key");
}

#[test]
/// 验证新旧行编码/解码与列映射。
fn TestRowCodec() {
    let _decimal_guard = DECIMAL_FAILPOINT_LOCK.lock().unwrap();
    let mut enum_type = field_type(mysql::TypeEnum);
    enum_type.SetElems(vec!["a".to_owned()]);
    let mut set_type = field_type(mysql::TypeSet);
    set_type.SetElems(vec!["a".to_owned()]);
    let mut bit_type = field_type(mysql::TypeBit);
    bit_type.SetFlen(8);

    let row = vec![
        types::NewIntDatum(100),
        types::NewBytesDatum(b"abc".to_vec()),
        types::NewDecimalDatum(decimal("1")),
        types::NewMysqlEnumDatum(types::Enum {
            Name: "a".to_owned(),
            Value: 1,
        }),
        types::NewMysqlSetDatum(
            types::Set {
                Name: "a".to_owned(),
                Value: 1,
            },
            String::new(),
        ),
        types::NewMysqlBitDatum(types::NewBinaryLiteralFromUint(100, 1)),
    ];
    let field_types = vec![
        field_type(mysql::TypeLonglong),
        field_type(mysql::TypeVarchar),
        field_type(mysql::TypeNewDecimal),
        enum_type,
        set_type,
        bit_type,
    ];
    let ids = vec![1, 2, 3, 4, 5, 6];
    let encoded = EncodeRow(
        Some(time::UTC),
        row.clone(),
        ids.clone(),
        Vec::new(),
        None,
        None,
        rowcodec::Encoder::new(true),
    )
    .unwrap();

    let full: HashMap<_, _> = ids
        .iter()
        .copied()
        .zip(field_types.iter().cloned())
        .collect();
    let decoded =
        DecodeRowToDatumMap(Some(encoded.clone()), full.clone(), Some(time::UTC)).unwrap();
    assert_eq!(decoded.len(), row.len());
    for (index, id) in ids.iter().enumerate() {
        assert_datum_equal(decoded.get(id).unwrap(), &row[index]);
    }

    let mut fewer = full;
    fewer.remove(&3);
    fewer.remove(&4);
    let decoded = DecodeRowToDatumMap(Some(encoded), fewer, Some(time::UTC)).unwrap();
    assert_eq!(decoded.len(), row.len() - 2);
    assert_datum_equal(decoded.get(&1).unwrap(), &row[0]);
    assert_datum_equal(decoded.get(&2).unwrap(), &row[1]);

    let empty = EncodeOldRow(Some(time::UTC), Vec::new(), Vec::new(), Vec::new(), None).unwrap();
    assert_eq!(empty, vec![codec::NilFlag]);
    assert!(
        DecodeRowToDatumMap(Some(empty), HashMap::new(), Some(time::UTC))
            .unwrap()
            .is_empty()
    );
}

#[test]
/// 验证单列值解码与类型还原。
fn TestDecodeColumnValue() {
    let time_context = types::BasicTimeContext {
        flags: types::TimeFlags::default(),
        location: time::UTC,
    };
    let timestamp = types::ParseTime(
        &time_context,
        "2026-07-14 12:34:56",
        mysql::TypeTimestamp,
        types::DefaultFsp,
    )
    .unwrap();
    let timestamp = types::NewTimeDatum(timestamp);
    let decoded = DecodeColumnValue(
        encode_old_value(timestamp.clone()),
        field_type(mysql::TypeTimestamp),
        Some(time::UTC),
    )
    .unwrap();
    assert_datum_equal(&decoded, &timestamp);

    let elements = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
    let set = types::NewMysqlSetDatum(types::ParseSetValue(&elements, 1).unwrap(), String::new());
    let mut set_type = field_type(mysql::TypeSet);
    set_type.SetElems(elements);
    assert_datum_equal(
        &DecodeColumnValue(encode_old_value(set.clone()), set_type, Some(time::UTC)).unwrap(),
        &set,
    );

    let bit = types::NewMysqlBitDatum(types::NewBinaryLiteralFromUint(3_223_600, 3));
    let mut bit_type = field_type(mysql::TypeBit);
    bit_type.SetFlen(24);
    assert_datum_equal(
        &DecodeColumnValue(encode_old_value(bit.clone()), bit_type, Some(time::UTC)).unwrap(),
        &bit,
    );

    let empty_enum = types::NewMysqlEnumDatum(types::Enum::default());
    assert_datum_equal(
        &DecodeColumnValue(
            encode_old_value(empty_enum.clone()),
            field_type(mysql::TypeEnum),
            Some(time::UTC),
        )
        .unwrap(),
        &empty_enum,
    );
}

#[test]
/// 验证 Unflatten 将原始 Datum 还原为列类型。
fn TestUnflattenDatums() {
    let input = vec![types::NewIntDatum(1)];
    let output = UnflattenDatums(
        input.clone(),
        vec![field_type(mysql::TypeLonglong)],
        Some(time::UTC),
    )
    .unwrap();
    assert_datum_equal(&output[0], &input[0]);

    let input = vec![types::NewCollationStringDatum(
        "aaa".to_owned(),
        "utf8mb4_unicode_ci".to_owned(),
    )];
    let mut blob = field_type(mysql::TypeBlob);
    blob.SetCollate("utf8mb4_unicode_ci".to_owned());
    let output = UnflattenDatums(input.clone(), vec![blob], Some(time::UTC)).unwrap();
    assert_datum_equal(&output[0], &input[0]);
    assert_eq!(output[0].Collation(), "utf8mb4_unicode_ci");
}

#[test]
/// 验证时间类型在行编码中的往返。
fn TestTimeCodec() {
    let context = types::BasicTimeContext {
        flags: types::TimeFlags::default(),
        location: time::UTC,
    };
    let timestamp =
        types::ParseTime(&context, "2016-06-23 11:30:45", mysql::TypeTimestamp, 0).unwrap();
    let (duration, _) = types::ParseDuration(&context, "12:59:59.999999", 6).unwrap();
    let row = vec![
        types::NewIntDatum(100),
        types::NewBytesDatum(b"abc".to_vec()),
        types::NewTimeDatum(timestamp),
        types::NewDurationDatum(duration),
    ];
    let mut duration_type = field_type(mysql::TypeDuration);
    duration_type.SetDecimal(6);
    let fields = vec![
        field_type(mysql::TypeLonglong),
        field_type(mysql::TypeVarchar),
        field_type(mysql::TypeTimestamp),
        duration_type,
    ];
    let ids = vec![1, 2, 3, 4];
    let encoded = EncodeRow(
        Some(time::UTC),
        row.clone(),
        ids.clone(),
        Vec::new(),
        None,
        None,
        rowcodec::Encoder::new(true),
    )
    .unwrap();
    let columns = ids.iter().copied().zip(fields).collect();
    let decoded = DecodeRowToDatumMap(Some(encoded), columns, Some(time::UTC)).unwrap();
    for (index, id) in ids.iter().enumerate() {
        assert_datum_equal(decoded.get(id).unwrap(), &row[index]);
    }
}

#[test]
/// 验证按列裁剪行编码数据。
fn TestCutRow() {
    let row = vec![
        types::NewIntDatum(100),
        types::NewBytesDatum(b"abc".to_vec()),
        types::NewDecimalDatum(decimal("1")),
    ];
    let expected: Vec<_> = row
        .iter()
        .cloned()
        .map(|value| EncodeValue(Some(time::UTC), Vec::new(), value).unwrap())
        .collect();
    let encoded = EncodeOldRow(Some(time::UTC), row, vec![1, 2, 3], Vec::new(), None).unwrap();
    let columns = HashMap::from([(1, 0), (2, 1), (3, 2)]);
    assert_eq!(CutRowNew(Some(encoded), columns.clone()).unwrap(), expected);
    assert!(
        CutRowNew(Some(vec![codec::NilFlag]), columns.clone())
            .unwrap()
            .is_empty()
    );
    assert!(CutRowNew(None, columns).unwrap().is_empty());
}

/// 构造带编码索引列的测试索引 key。
fn encoded_index_fixture() -> (Vec<types::Datum>, kv::Key) {
    let values = vec![
        types::NewIntDatum(1),
        types::NewBytesDatum(b"abc".to_vec()),
        types::NewFloat64Datum(5.5),
        types::NewIntDatum(100),
    ];
    let encoded = codec::EncodeKey(time::UTC, Vec::new(), values.clone()).unwrap();
    (values, EncodeIndexSeekKey(4, 5, Some(encoded)))
}

#[test]
/// 验证 CutIndexKeyNew 切割索引前缀与列值。
fn TestCutKeyNew() {
    let (values, key) = encoded_index_fixture();
    let (cut, handle) = CutIndexKeyNew(key, 3).unwrap();
    for index in 0..3 {
        assert_datum_equal(&codec::DecodeOne(&cut[index]).unwrap().1, &values[index]);
    }
    assert_datum_equal(&codec::DecodeOne(&handle).unwrap().1, &values[3]);
}

#[test]
/// 验证 CutIndexKey 切割行为。
fn TestCutKey() {
    let (values, key) = encoded_index_fixture();
    let ids = vec![1, 2, 3];
    let (cut, handle) = CutIndexKey(key, ids.clone()).unwrap();
    for (index, id) in ids.iter().enumerate() {
        assert_datum_equal(
            &codec::DecodeOne(cut.get(id).unwrap()).unwrap().1,
            &values[index],
        );
    }
    assert_datum_equal(&codec::DecodeOne(&handle).unwrap().1, &values[3]);
}

#[test]
/// 验证损坏的 decimal 字节触发预期错误（含 failpoint）。
fn TestDecodeBadDecical() {
    let _decimal_guard = DECIMAL_FAILPOINT_LOCK.lock().unwrap();
    let _scenario = fail::FailScenario::setup();
    let bytes = codec::EncodeValue(
        time::UTC,
        Vec::new(),
        vec![types::NewDecimalDatum(decimal("0.111"))],
    )
    .unwrap();
    fail::cfg("errorInDecodeDecimal", "return").unwrap();
    assert!(codec::DecodeOne(&bytes).is_err());
    fail::remove("errorInDecodeDecimal");
}

#[test]
/// 验证索引 seek key 编码。
fn TestIndexKey() {
    assert_eq!(
        DecodeKeyHead(EncodeIndexSeekKey(4, 5, None)).unwrap(),
        (4, 5, false)
    );
}

#[test]
/// 验证记录 key 与 handle 编解码。
fn TestRecordKey() {
    let table_id = 55;
    let table_key = EncodeRowKeyWithHandle(table_id, Box::new(kv::IntHandle(u32::MAX as i64)));
    assert_eq!(
        DecodeKeyHead(table_key.clone()).unwrap(),
        (table_id, 0, true)
    );
    let row_key = EncodeRowKey(table_id, &codec::EncodeInt(Vec::new(), u32::MAX as i64));
    assert_eq!(row_key, table_key);
    let (decoded_table, handle) = DecodeRecordKey(row_key.clone()).unwrap();
    assert_eq!(
        (decoded_table, handle.IntValue()),
        (table_id, u32::MAX as i64)
    );
    assert_eq!(
        EncodeRecordKey(
            GenTableRecordPrefix(table_id),
            Box::new(kv::IntHandle(u32::MAX as i64))
        ),
        table_key
    );
    assert!(DecodeRecordKey(kv::Key(Vec::new())).is_err());
    assert!(DecodeRecordKey(kv::Key(b"abcdefghijklmnopqrstuvwxyz".to_vec())).is_err());
    assert_eq!(DecodeTableID(kv::Key(Vec::new())), 0);
}

#[test]
/// 验证表/记录/索引前缀生成与识别。
fn TestPrefix() {
    let table_id = 66;
    let key = EncodeTablePrefix(table_id);
    assert_eq!(DecodeTableID(key.clone()), table_id);
    assert_eq!(TablePrefix(), b"t");
    assert_eq!(GenTablePrefix(table_id), key);
    let index = EncodeTableIndexPrefix(table_id, u32::MAX as i64);
    assert_eq!(
        DecodeKeyHead(index.clone()).unwrap(),
        (table_id, u32::MAX as i64, false)
    );
    assert_eq!(DecodeTableID(GenTableIndexPrefix(table_id)), table_id);
    let mut long = index.0;
    long.extend_from_slice(b"xyz");
    assert_eq!(TruncateToRowKeyLen(kv::Key(long)).0.len(), RecordRowKeyLen);
    assert_eq!(TruncateToRowKeyLen(key.clone()).0.len(), key.0.len());
}

#[test]
/// 验证索引 key 解码出 table/index id 与列值。
fn TestDecodeIndexKey() {
    let values = vec![
        types::NewIntDatum(1),
        types::NewBytesDatum(b"abc".to_vec()),
        types::NewFloat64Datum(123.45),
    ];
    let expected: Vec<_> = values
        .iter()
        .map(|value| value.ToString().unwrap())
        .collect();
    let encoded = codec::EncodeKey(time::UTC, Vec::new(), values).unwrap();
    assert_eq!(
        DecodeIndexKey(EncodeIndexSeekKey(4, 5, Some(encoded))).unwrap(),
        (4, 5, expected)
    );
}

#[test]
/// 验证裁剪行/索引前缀。
fn TestCutPrefix() {
    let key = EncodeTableIndexPrefix(42, 666);
    assert_eq!(
        CutRowKeyPrefix(key.clone()),
        vec![0x80, 0, 0, 0, 0, 0, 2, 0x9a]
    );
    assert!(CutIndexPrefix(key).is_empty());
}

#[test]
/// 验证表 handle/索引的 key range 边界。
fn TestRange() {
    let (s1, e1) = GetTableHandleKeyRange(22);
    let (s2, e2) = GetTableHandleKeyRange(23);
    assert!(s1 < e1 && e1 < s2 && s2 < e2);
    let (s1, e1) = GetTableIndexKeyRange(42, 666);
    let (s2, e2) = GetTableIndexKeyRange(42, 667);
    assert!(s1 < e1 && e1 < s2 && s2 < e2);
}

#[test]
/// 验证 meta key（含自增 ID）编解码。
fn TestDecodeAutoIDMeta() {
    let bytes = vec![
        0x6d, 0x44, 0x42, 0x3a, 0x35, 0x36, 0, 0, 0, 0xfc, 0, 0, 0, 0, 0, 0, 0, 0x68, 0x54, 0x49,
        0x44, 0x3a, 0x31, 0x30, 0x38, 0, 0xfe,
    ];
    let (key, field) = DecodeMetaKey(kv::Key(bytes)).unwrap();
    assert_eq!(key, b"DB:56");
    assert_eq!(field, b"TID:108");
}

/// 基准：手写 hasTablePrefix 检查。
pub fn BenchmarkHasTablePrefix() {
    assert!(GenTablePrefix(1).as_ref().starts_with(TablePrefix()));
}

/// 基准：内置前缀检查对比。
pub fn BenchmarkHasTablePrefixBuiltin() {
    assert!(
        kv::Key(b"table".to_vec())
            .as_ref()
            .starts_with(TablePrefix())
    );
}

/// 基准：EncodeValue 编码开销。
pub fn BenchmarkEncodeValue() {
    let row = vec![
        types::NewIntDatum(100),
        types::NewBytesDatum(b"abc".to_vec()),
        types::NewDecimalDatum(decimal("1")),
        types::NewMysqlEnumDatum(types::Enum {
            Name: "a".to_owned(),
            Value: 0,
        }),
        types::NewMysqlSetDatum(
            types::Set {
                Name: "a".to_owned(),
                Value: 0,
            },
            String::new(),
        ),
        types::NewMysqlBitDatum(types::NewBinaryLiteralFromUint(100, 1)),
        types::NewFloat32Datum(1.5),
    ];
    for datum in row {
        assert!(!EncodeValue(None, Vec::new(), datum).unwrap().is_empty());
    }
}

#[test]
/// 验证错误构造函数返回正确错误码。
fn TestError() {
    for error in [errInvalidKey(), errInvalidRecordKey(), errInvalidIndexKey()] {
        let sql_error = terror::ToSQLError(error.as_ref());
        assert_ne!(sql_error.Code, 1105);
        assert_eq!(sql_error.Code, error.Code() as u16);
    }
}

#[test]
/// 验证 untouched 索引键值判定。
fn TestUntouchedIndexKValue() {
    let key = b"t00000001_i000000001".to_vec();
    let untouched = vec![0, 0, 0, 0, 0, 0, 0, 1, b'1'];
    assert!(IsUntouchedIndexKValue(&key, &untouched));
    assert!(!IsUntouchedIndexKValue(&key, &[0, IndexVersionFlag, 1]));
    assert!(IsUntouchedIndexKValue(
        &key,
        &[1, IndexVersionFlag, 1, kv::UnCommitIndexKVFlag]
    ));
    let legacy = EncodeHandleInUniqueIndexValue(Box::new(kv::IntHandle(0x017d010000000031)), false);
    assert_eq!(legacy.len(), 8);
    assert_eq!(legacy[1], IndexVersionFlag);
    assert_eq!(*legacy.last().unwrap(), kv::UnCommitIndexKVFlag);
    assert!(!IsUntouchedIndexKValue(&key, &legacy));
    let mut temp_key = key.clone();
    IndexKey2TempIndexKey(&mut temp_key);
    assert!(IsUntouchedIndexKValue(&temp_key, &untouched));
    let temp_value =
        temp_elem(Vec::new(), Box::new(kv::IntHandle(1)), b'b', true, true).Encode(None);
    assert!(!IsUntouchedIndexKValue(&temp_key, &temp_value));
}

#[test]
/// 验证临时索引 key 与普通索引 key 互转。
fn TestTempIndexKey() {
    let values = vec![
        types::NewIntDatum(1),
        types::NewBytesDatum(b"abc".to_vec()),
        types::NewFloat64Datum(5.5),
    ];
    let encoded = codec::EncodeKey(time::UTC, Vec::new(), values).unwrap();
    let mut key = EncodeIndexSeekKey(4, 5, Some(encoded));
    IndexKey2TempIndexKey(&mut key.0);
    let (table_id, temp_id, _) = DecodeKeyHead(key.clone()).unwrap();
    assert_eq!(table_id, 4);
    assert_ne!(temp_id, 5);
    assert_eq!(temp_id & IndexIDMask, 5);
    assert_eq!(DecodeIndexID(key.clone()).unwrap(), temp_id);
    TempIndexKey2IndexKey(&mut key.0);
    assert_eq!(DecodeKeyHead(key.clone()).unwrap(), (4, 5, false));
    assert_eq!(DecodeIndexID(key).unwrap(), 5);
}

#[test]
/// 验证临时索引 value 多元素编解码。
fn TestTempIndexValueCodec() {
    assert!(DecodeTempIndexValue(vec![b'0']).is_err());
    let raw_index_value = temp_elem(
        vec![b'0'],
        Box::new(kv::IntHandle(0)),
        TempIndexKeyTypeBackfill,
        false,
        false,
    );
    let decoded_raw_index_value = DecodeTempIndexValue(raw_index_value.Encode(None)).unwrap();
    assert_eq!(decoded_raw_index_value.len(), 1);
    assert_eq!(
        decoded_raw_index_value[0].as_ref().unwrap().Value,
        vec![b'0']
    );

    let encoded = codec::EncodeValue(time::UTC, Vec::new(), vec![types::NewIntDatum(1)]).unwrap();
    let cases = vec![
        temp_elem(encoded, Box::new(kv::IntHandle(0)), b'b', false, false),
        temp_elem(
            EncodeHandleInUniqueIndexValue(Box::new(kv::IntHandle(100)), false),
            Box::new(kv::IntHandle(0)),
            b'm',
            false,
            true,
        ),
        temp_elem(Vec::new(), Box::new(kv::IntHandle(0)), b'b', true, false),
        temp_elem(Vec::new(), Box::new(kv::IntHandle(100)), b'b', true, true),
    ];
    for source in cases {
        let mut decoded = temp_elem(Vec::new(), Box::new(kv::IntHandle(0)), 0, false, false);
        assert!(decoded.DecodeOne(source.Encode(None)).unwrap().is_empty());
        assert_eq!(decoded.Value, source.Value);
        assert_eq!(decoded.KeyVer, source.KeyVer);
        assert_eq!(decoded.Delete, source.Delete);
        assert_eq!(decoded.Distinct, source.Distinct);
        if source.Delete && source.Distinct {
            assert_eq!(decoded.Handle.IntValue(), source.Handle.IntValue());
        }
    }

    let first = temp_elem(
        EncodeHandleInUniqueIndexValue(Box::new(kv::IntHandle(100)), false),
        Box::new(kv::IntHandle(0)),
        b'm',
        false,
        true,
    );
    let second = temp_elem(Vec::new(), Box::new(kv::IntHandle(100)), b'm', true, true);
    let third = temp_elem(
        EncodeHandleInUniqueIndexValue(Box::new(kv::IntHandle(101)), false),
        Box::new(kv::IntHandle(0)),
        b'm',
        false,
        true,
    );
    let bytes = third.Encode(Some(second.Encode(Some(first.Encode(None)))));
    let decoded = DecodeTempIndexValue(bytes).unwrap();
    assert_eq!(decoded.len(), 3);
    let handles: Vec<_> = decoded
        .iter()
        .map(|entry| {
            let entry = entry.as_ref().unwrap();
            if entry.Delete {
                entry.Handle.IntValue()
            } else {
                DecodeHandleInIndexValue(entry.Value.clone())
                    .unwrap()
                    .expect("temporary index value handle")
                    .IntValue()
            }
        })
        .collect();
    assert_eq!(handles, vec![100, 100, 101]);

    let filtered = decoded.FilterOverwritten();
    let filtered_handles: Vec<_> = filtered
        .iter()
        .flatten()
        .map(|entry| entry.Handle.IntValue())
        .collect();
    assert_eq!(filtered_handles, vec![100, 101]);

    let deleted = temp_elem(Vec::new(), Box::new(kv::IntHandle(100)), b'b', true, true);
    assert!(!IndexKVIsUnique(deleted.Encode(None)));
}

#[test]
/// 验证 v2 行编解码路径。
fn TestV2TableCodec() {
    let table_id = 31_415_926;
    let mut key = b"x001".to_vec();
    key.extend_from_slice(EncodeTablePrefix(table_id).as_ref());
    assert_eq!(DecodeTableID(kv::Key(key)), table_id);
    assert_eq!(DecodeTableID(kv::Key(b"x001HelloWorld".to_vec())), 0);
    assert_eq!(DecodeTableID(kv::Key(b"x001x001t123".to_vec())), 0);
}

#[test]
/// 验证全局索引在 key/value 中携带 partition id 时的 handle 解码。
fn TestDecodeIndexHandleWithPartitionIDInKeyAndValue() {
    let encoded = codec::EncodeKey(time::UTC, Vec::new(), vec![types::NewIntDatum(123)]).unwrap();
    let mut key = EncodeIndexSeekKey(100, 1, Some(encoded)).0;
    key.push(PartitionIDFlag);
    key = codec::EncodeInt(key, 42);
    key.push(codec::IntHandleFlag);
    key = codec::EncodeInt(key, 999);

    let mut value = vec![0, PartitionIDFlag];
    value = codec::EncodeInt(value, 42);
    value.resize(10, 0);
    value[0] = (value.len() - 10) as u8;
    let handle = DecodeIndexHandle(key, value, 1)
        .unwrap()
        .expect("global index value must contain a handle");
    let partition = handle
        .as_any()
        .downcast_ref::<kv::PartitionHandle>()
        .expect("partition handle");
    assert_eq!(partition.PartitionID, 42);
    assert!(
        partition
            .Handle
            .as_any()
            .downcast_ref::<kv::PartitionHandle>()
            .is_none()
    );
    assert_eq!(partition.Handle.IntValue(), 999);
}

#[test]
/// 新版唯一索引 value 可以只携带 restored data 而不携带 handle；Go 返回 nil, nil。
fn TestDecodeHandleInIndexValueWithoutHandleReturnsNone() {
    // version-0 新格式：首字节 tailLen=0，总长度大于旧格式上限，且没有任何 handle flag。
    let value = vec![0; MaxOldEncodeValueLen + 1];
    assert!(DecodeHandleInIndexValue(value).unwrap().is_none());
}

#[test]
/// 唯一索引 value 未携带 handle 时，DecodeIndexHandle 对齐 Go 返回 nil, nil。
fn TestDecodeIndexHandleWithoutHandleReturnsNone() {
    let encoded = codec::EncodeKey(time::UTC, Vec::new(), vec![types::NewIntDatum(123)]).unwrap();
    let key = EncodeIndexSeekKey(100, 1, Some(encoded)).0;
    let value = vec![0; MaxOldEncodeValueLen + 1];
    assert!(DecodeIndexHandle(key, value, 1).unwrap().is_none());
}

/// 构造指定版本的全局唯一索引表元数据夹具。
fn global_index_fixture(version: u8) -> (Box<model::TableInfo>, Box<model::IndexInfo>) {
    let mut first = model::ColumnInfo::default();
    first.ID = 1;
    first.Offset = 0;
    first.FieldType.SetType(mysql::TypeLong);
    let mut second = model::ColumnInfo::default();
    second.ID = 2;
    second.Offset = 1;
    second.FieldType.SetType(mysql::TypeLong);
    let table = model::TableInfo {
        Columns: vec![first, second],
        Indices: Vec::new(),
        PKIsHandle: false,
        IsCommonHandle: false,
        CommonHandleVersion: 0,
        ..Default::default()
    };
    let index = model::IndexInfo {
        ID: 1,
        Columns: vec![model::IndexColumn {
            Offset: 1,
            Length: types::UnspecifiedLength as isize,
            ..Default::default()
        }],
        Unique: true,
        Global: true,
        GlobalIndexVersion: version,
        ..Default::default()
    };
    (Box::new(table), Box::new(index))
}

#[test]
/// 验证含 NULL 列的全局唯一索引 key 生成。
fn TestUniqueGlobalIndexKeyWithNullValues() {
    let (table, index) = global_index_fixture(model::GlobalIndexVersionV1);
    let (key, distinct) = GenIndexKey(
        defaultCodecEncoder(),
        Some(time::UTC),
        table.clone(),
        index.clone(),
        100,
        vec![types::NewIntDatum(123)],
        Some(Box::new(kv::NewPartitionHandle(
            42,
            Box::new(kv::IntHandle(999)),
        ))),
        None,
    )
    .unwrap();
    assert!(distinct);
    assert!(!key.contains(&PartitionIDFlag));

    let (key, distinct) = GenIndexKey(
        defaultCodecEncoder(),
        Some(time::UTC),
        table.clone(),
        index.clone(),
        100,
        vec![types::Datum::default()],
        Some(Box::new(kv::NewPartitionHandle(
            42,
            Box::new(kv::IntHandle(999)),
        ))),
        None,
    )
    .unwrap();
    assert!(!distinct);
    let position = key
        .iter()
        .position(|byte| *byte == PartitionIDFlag)
        .unwrap();
    assert_eq!(
        codec::DecodeCmpUintToInt(u64::from_be_bytes(
            key[position + 1..position + 9].try_into().unwrap()
        )),
        42
    );

    let value = genIndexValueVersion0(
        Some(time::UTC),
        table,
        index,
        false,
        true,
        false,
        vec![types::NewIntDatum(123)],
        Box::new(kv::IntHandle(999)),
        42,
        None,
    )
    .unwrap();
    assert!(value.contains(&PartitionIDFlag));

    let (table, legacy) = global_index_fixture(0);
    let (key, distinct) = GenIndexKey(
        defaultCodecEncoder(),
        Some(time::UTC),
        table,
        legacy,
        100,
        vec![types::Datum::default()],
        Some(Box::new(kv::IntHandle(999))),
        None,
    )
    .unwrap();
    assert!(!distinct);
    assert!(!key.contains(&PartitionIDFlag));
}
