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

// rowcodec 编解码集成单测：多类型 Encode/Decode、handle、默认值、校验和与旧格式兼容。
//
// 覆盖 DatumMap / Chunk / Bytes 三条解码路径，以及 large/small、checksum、commitTS 等边界。

#![allow(non_snake_case)]

use super::*;
use std::collections::HashMap;

/// 单列测试夹具：列 ID、字段类型、输入/期望 Datum、可选默认值与是否为 handle。
#[derive(Clone)]
struct TestData {
    id: i64,
    ft: types::FieldType,
    input: types::Datum,
    output: types::Datum,
    default: Option<types::Datum>,
    handle: bool,
}

/// 按 MySQL 类型码构造 `FieldType`。
fn field_type(tp: u8) -> types::FieldType {
    *types::NewFieldType(tp)
}

/// 为字段类型加上 UNSIGNED 标志。
fn unsigned(mut ft: types::FieldType) -> types::FieldType {
    ft.AddFlag(mysql::UnsignedFlag);
    ft
}

/// 设置字段 decimal（精度/小数位相关）。
fn with_decimal(mut ft: types::FieldType, decimal: isize) -> types::FieldType {
    ft.SetDecimal(decimal);
    ft
}

/// 设置字段显示长度 flen。
fn with_flen(mut ft: types::FieldType, flen: isize) -> types::FieldType {
    ft.SetFlen(flen);
    ft
}

/// 设置 ENUM/SET 的元素列表。
fn with_elems(mut ft: types::FieldType, elems: &[&str]) -> types::FieldType {
    ft.SetElems(elems.iter().map(|value| (*value).to_owned()).collect());
    ft
}

/// 从字符串解析 `MyDecimal` 夹具。
fn decimal(value: &str) -> types::MyDecimal {
    let mut result = types::MyDecimal::default();
    result
        .FromString(value.as_bytes())
        .expect("valid decimal fixture");
    result
}

/// 构造带 length/frac 的 Decimal Datum。
fn decimal_datum(value: &str, length: i32, frac: i32) -> types::Datum {
    let mut datum = types::NewDecimalDatum(decimal(value));
    datum.SetLength(length);
    datum.SetFrac(frac);
    datum
}

/// 解析 TIMESTAMP 字符串为 `Time`（fsp=6）。
fn parse_time(value: &str) -> types::Time {
    types::ParseTime(
        &types::BasicTimeContext::default(),
        value,
        mysql::TypeTimestamp,
        6,
    )
    .expect("valid time fixture")
}

/// 解析 Duration 字符串。
fn parse_duration(value: &str) -> types::Duration {
    types::ParseDuration(&types::BasicTimeContext::default(), value, 0)
        .expect("valid duration fixture")
        .0
}

/// 由 `TestData` 生成解码用 `ColInfo`。
fn col_info(data: &TestData) -> rowcodec::ColInfo {
    rowcodec::ColInfo {
        ID: data.id,
        IsPKHandle: data.handle,
        VirtualGenCol: false,
        Ft: data.ft.clone(),
    }
}

/// 按 Datum Kind 逐字段断言期望值与实际值一致。
fn assert_datum(expected: &types::Datum, actual: &types::Datum) {
    assert_eq!(expected.Kind(), actual.Kind(), "datum kind mismatch");
    match expected.Kind() {
        types::KindNull => assert!(actual.IsNull()),
        types::KindInt64 => assert_eq!(expected.GetInt64(), actual.GetInt64()),
        types::KindUint64 => assert_eq!(expected.GetUint64(), actual.GetUint64()),
        types::KindFloat32 => {
            assert_eq!(
                expected.GetFloat32().to_bits(),
                actual.GetFloat32().to_bits()
            )
        }
        types::KindFloat64 => {
            assert_eq!(
                expected.GetFloat64().to_bits(),
                actual.GetFloat64().to_bits()
            )
        }
        types::KindString => assert_eq!(expected.GetString(), actual.GetString()),
        types::KindBytes => assert_eq!(expected.GetBytes(), actual.GetBytes()),
        types::KindMysqlDecimal => {
            assert_eq!(
                expected.GetMysqlDecimal().String(),
                actual.GetMysqlDecimal().String()
            )
        }
        types::KindMysqlDuration => {
            assert_eq!(expected.GetMysqlDuration(), actual.GetMysqlDuration())
        }
        types::KindMysqlEnum => assert_eq!(expected.GetMysqlEnum(), actual.GetMysqlEnum()),
        types::KindMysqlSet => assert_eq!(expected.GetMysqlSet(), actual.GetMysqlSet()),
        types::KindMysqlBit | types::KindBinaryLiteral => {
            assert_eq!(expected.GetBinaryLiteral().0, actual.GetBinaryLiteral().0)
        }
        types::KindMysqlTime => {
            assert_eq!(
                expected.GetMysqlTime().String(),
                actual.GetMysqlTime().String()
            )
        }
        types::KindMysqlJSON => {
            assert_eq!(
                expected.GetMysqlJSON().String(),
                actual.GetMysqlJSON().String()
            )
        }
        kind => panic!("unhandled datum kind {kind}"),
    }
}

/// 从夹具提取待编码的列 ID 与输入 Datum（可跳过默认值列与 handle 列）。
fn fixture_inputs(data: &[TestData], skip_defaults: bool) -> (Vec<i64>, Vec<types::Datum>) {
    data.iter()
        .filter(|item| !(skip_defaults && item.default.is_some()) && !item.handle)
        .map(|item| (item.id, item.input.clone()))
        .unzip()
}

/// 经 ChunkDecoder 解码一行并返回各列 Datum。
fn decode_chunk(
    data: &[TestData],
    encoded: &[u8],
    handle_ids: Vec<i64>,
    handle: Option<&dyn kv::Handle>,
    commit_ts: u64,
    default: Option<
        Box<dyn Fn(usize, &mut chunk::Chunk) -> Result<(), rowcodec_errors::SharedError>>,
    >,
) -> Vec<types::Datum> {
    let columns = data.iter().map(col_info).collect();
    let fields: Vec<_> = data.iter().map(|item| item.ft.clone()).collect();
    let mut decoder = rowcodec::NewChunkDecoder(columns, handle_ids, default, Some(time::UTC));
    let mut chunk = chunk::NewChunkWithCapacity(fields.clone(), 1);
    decoder
        .DecodeToChunk(encoded, commit_ts, handle, &mut chunk)
        .expect("chunk decode should succeed");
    assert_eq!(1, chunk.NumRows());
    chunk.GetRow(0).GetDatumRow(&fields)
}

/// 经 BytesDecoder 解成旧 datum 字节再 DecodeOne 为 Datum 列表。
fn decode_old_row(
    data: &[TestData],
    encoded: &[u8],
    handle_ids: Vec<i64>,
    handle: &dyn kv::Handle,
    default: Option<Box<dyn Fn(usize) -> Result<Vec<u8>, rowcodec_errors::SharedError>>>,
) -> Vec<types::Datum> {
    let offsets: HashMap<_, _> = data
        .iter()
        .enumerate()
        .map(|(index, item)| (item.id, index))
        .collect();
    let decoder = rowcodec::NewByteDecoder(
        data.iter().map(col_info).collect(),
        handle_ids,
        default,
        Some(time::UTC),
    );
    decoder
        .DecodeToBytes(&offsets, handle, encoded, &[])
        .expect("byte decode should succeed")
        .iter()
        .map(|bytes| {
            let (remaining, datum) = codec::DecodeOne(bytes).expect("old datum should decode");
            assert!(remaining.is_empty());
            datum
        })
        .collect()
}

/// 复用 Encoder 时 large→small 布局切换不得污染后续编码。
#[test]
fn TestEncodeLargeSmallReuseBug() {
    let mut encoder = rowcodec::Encoder::new(true);
    let large = encoder
        .Encode(
            None,
            vec![300],
            vec![types::NewBytesDatum(Vec::new())],
            None,
            Vec::new(),
        )
        .expect("large column encode");
    let mut decoder = rowcodec::NewDatumMapDecoder(
        vec![rowcodec::ColInfo {
            ID: 300,
            Ft: field_type(mysql::TypeString),
            IsPKHandle: false,
            VirtualGenCol: false,
        }],
        None,
    );
    decoder
        .DecodeToDatumMap(&large, None)
        .expect("large column decode");

    let small = encoder
        .Encode(None, vec![1], vec![types::NewIntDatum(2)], None, Vec::new())
        .expect("small column encode");
    let mut decoder = rowcodec::NewDatumMapDecoder(
        vec![rowcodec::ColInfo {
            ID: 1,
            Ft: field_type(mysql::TypeLonglong),
            IsPKHandle: false,
            VirtualGenCol: false,
        }],
        None,
    );
    let decoded = decoder
        .DecodeToDatumMap(&small, None)
        .expect("small column decode");
    assert_eq!(2, decoded[&1].GetInt64());
}

/// 带主键 handle 的行解码：value 缺列时从 handle 补值。
#[test]
fn TestDecodeRowWithHandle() {
    let handle_id = -1;
    let handle_value = 10_000;
    for (name, handle_type, expected) in [
        (
            "signed int",
            field_type(mysql::TypeLonglong),
            types::NewIntDatum(handle_value),
        ),
        (
            "unsigned int",
            unsigned(field_type(mysql::TypeLonglong)),
            types::NewUintDatum(handle_value as u64),
        ),
    ] {
        let data = vec![
            TestData {
                id: handle_id,
                ft: handle_type,
                input: expected.clone(),
                output: expected,
                default: None,
                handle: true,
            },
            TestData {
                id: 10,
                ft: field_type(mysql::TypeLonglong),
                input: types::NewIntDatum(1),
                output: types::NewIntDatum(1),
                default: None,
                handle: false,
            },
        ];
        let (ids, datums) = fixture_inputs(&data, false);
        let encoded = rowcodec::Encoder::new(true)
            .Encode(Some(&time::UTC), ids, datums, None, Vec::new())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let mut decoder =
            rowcodec::NewDatumMapDecoder(data.iter().map(col_info).collect(), Some(time::UTC));
        let decoded = decoder
            .DecodeToDatumMap(&encoded, None)
            .expect("datum map decode");
        let column_types = HashMap::from([(handle_id, Box::new(data[0].ft.clone()))]);
        let decoded = tablecodec::DecodeHandleToDatumMap(
            Some(Box::new(tablecodec::kv::IntHandle(handle_value))),
            vec![handle_id],
            column_types,
            Some(time::UTC),
            Some(decoded),
        )
        .expect("handle map decode");
        for item in &data {
            assert_datum(&item.input, &decoded[&item.id]);
        }

        let handle = kv::IntHandle(handle_value);
        for (expected, actual) in data.iter().zip(decode_chunk(
            &data,
            &encoded,
            vec![handle_id],
            Some(&handle),
            0,
            None,
        )) {
            assert_datum(&expected.output, &actual);
        }
        for (expected, actual) in data.iter().zip(decode_old_row(
            &data,
            &encoded,
            vec![handle_id],
            &handle,
            None,
        )) {
            assert_datum(&expected.output, &actual);
        }
    }
}

/// KindNull Datum 编码后解码仍为 NULL。
#[test]
fn TestEncodeKindNullDatum() {
    let ft = field_type(mysql::TypeLonglong);
    let data = vec![
        TestData {
            id: 1,
            ft: ft.clone(),
            input: types::Datum::default(),
            output: types::Datum::default(),
            default: None,
            handle: false,
        },
        TestData {
            id: 2,
            ft,
            input: types::NewIntDatum(2),
            output: types::NewIntDatum(2),
            default: None,
            handle: false,
        },
    ];
    let (ids, datums) = fixture_inputs(&data, false);
    let encoded = rowcodec::Encoder::new(true)
        .Encode(Some(&time::UTC), ids, datums, None, Vec::new())
        .expect("encode");
    let decoded = decode_chunk(&data, &encoded, vec![-1], Some(&kv::IntHandle(-1)), 0, None);
    assert!(decoded[0].IsNull());
    assert_eq!(2, decoded[1].GetInt64());
}

/// Decimal 存储 fsp 与字段 decimal 不一致时的解码/四舍五入行为。
#[test]
fn TestDecodeDecimalFspNotMatch() {
    let encoded = rowcodec::Encoder::new(true)
        .Encode(
            Some(&time::UTC),
            vec![1],
            vec![decimal_datum("11.9900", 6, 4)],
            None,
            Vec::new(),
        )
        .expect("encode decimal");
    let data = vec![TestData {
        id: 1,
        ft: with_decimal(field_type(mysql::TypeNewDecimal), 3),
        input: decimal_datum("11.990", 6, 3),
        output: decimal_datum("11.990", 6, 3),
        default: None,
        handle: false,
    }];
    let decoded = decode_chunk(&data, &encoded, vec![-1], Some(&kv::IntHandle(-1)), 0, None);
    assert_eq!("11.990", decoded[0].GetMysqlDecimal().String());
}

/// 覆盖主要 MySQL 类型的 Encode/Decode 夹具集。
fn typed_fixture() -> Vec<TestData> {
    let mut blob = field_type(mysql::TypeBlob);
    blob.SetCollate("utf8mb4_bin".to_owned());
    blob.SetFlen(types::UnspecifiedLength as isize);
    let mut string = field_type(mysql::TypeString);
    string.SetCollate("utf8mb4_bin".to_owned());
    let mut enumeration = with_elems(field_type(mysql::TypeEnum), &["y", "n"]);
    enumeration.SetCollate("utf8mb4_bin".to_owned());
    enumeration.SetFlen(collate::DefaultLen as isize);
    let mut set = with_elems(field_type(mysql::TypeSet), &["n1", "n2"]);
    set.SetCollate("utf8mb4_bin".to_owned());
    set.SetFlen(collate::DefaultLen as isize);
    let mut var_string = field_type(mysql::TypeVarString);
    var_string.SetCollate("utf8mb4_bin".to_owned());

    let json = types::ParseBinaryJSONFromString(r#"{"a":2}"#).expect("json fixture");
    vec![
        TestData {
            id: 1,
            ft: field_type(mysql::TypeLonglong),
            input: types::NewIntDatum(1),
            output: types::NewIntDatum(1),
            default: None,
            handle: false,
        },
        TestData {
            id: 22,
            ft: unsigned(field_type(mysql::TypeShort)),
            input: types::NewUintDatum(1),
            output: types::NewUintDatum(1),
            default: None,
            handle: false,
        },
        TestData {
            id: 3,
            ft: field_type(mysql::TypeDouble),
            input: types::NewFloat64Datum(2.0),
            output: types::NewFloat64Datum(2.0),
            default: None,
            handle: false,
        },
        TestData {
            id: 24,
            ft: blob,
            input: types::NewStringDatum("abc".to_owned()),
            output: types::NewBytesDatum(b"abc".to_vec()),
            default: None,
            handle: false,
        },
        TestData {
            id: 25,
            ft: string,
            input: types::NewStringDatum("ab".to_owned()),
            output: types::NewBytesDatum(b"ab".to_vec()),
            default: None,
            handle: false,
        },
        TestData {
            id: 5,
            ft: with_decimal(field_type(mysql::TypeTimestamp), 6),
            input: types::NewTimeDatum(parse_time("2011-11-10 11:11:11.999999")),
            output: types::NewUintDatum(1_840_446_893_366_133_311),
            default: None,
            handle: false,
        },
        TestData {
            id: 16,
            ft: with_decimal(field_type(mysql::TypeDuration), 0),
            input: types::NewDurationDatum(parse_duration("4:00:00")),
            output: types::NewIntDatum(14_400_000_000_000),
            default: None,
            handle: false,
        },
        TestData {
            id: 8,
            ft: field_type(mysql::TypeNewDecimal),
            input: decimal_datum("11.9900", 6, 4),
            output: decimal_datum("11.9900", 6, 4),
            default: None,
            handle: false,
        },
        TestData {
            id: 12,
            ft: field_type(mysql::TypeYear),
            input: types::NewIntDatum(1999),
            output: types::NewIntDatum(1999),
            default: None,
            handle: false,
        },
        TestData {
            id: 9,
            ft: enumeration,
            input: types::NewMysqlEnumDatum(types::Enum {
                Name: "n".to_owned(),
                Value: 2,
            }),
            output: types::NewUintDatum(2),
            default: None,
            handle: false,
        },
        TestData {
            id: 14,
            ft: field_type(mysql::TypeJSON),
            input: types::NewJSONDatum(json.clone()),
            output: types::NewJSONDatum(json),
            default: None,
            handle: false,
        },
        TestData {
            id: 11,
            ft: field_type(mysql::TypeNull),
            input: types::Datum::default(),
            output: types::Datum::default(),
            default: None,
            handle: false,
        },
        TestData {
            id: 2,
            ft: field_type(mysql::TypeNull),
            input: types::Datum::default(),
            output: types::Datum::default(),
            default: None,
            handle: false,
        },
        TestData {
            id: 100,
            ft: field_type(mysql::TypeNull),
            input: types::Datum::default(),
            output: types::Datum::default(),
            default: None,
            handle: false,
        },
        TestData {
            id: 116,
            ft: field_type(mysql::TypeFloat),
            input: types::NewFloat32Datum(6.0),
            output: types::NewFloat64Datum(6.0),
            default: None,
            handle: false,
        },
        TestData {
            id: 117,
            ft: set,
            input: types::NewMysqlSetDatum(
                types::Set {
                    Name: "n1".to_owned(),
                    Value: 1,
                },
                "utf8mb4_bin".to_owned(),
            ),
            output: types::NewUintDatum(1),
            default: None,
            handle: false,
        },
        TestData {
            id: 118,
            ft: with_flen(field_type(mysql::TypeBit), 24),
            input: types::NewMysqlBitDatum(types::NewBinaryLiteralFromUint(3_223_600, 3)),
            output: types::NewUintDatum(3_223_600),
            default: None,
            handle: false,
        },
        TestData {
            id: 119,
            ft: var_string,
            input: types::NewStringDatum(String::new()),
            output: types::NewBytesDatum(Vec::new()),
            default: None,
            handle: false,
        },
    ]
}

/// 对同一编码结果走 DatumMap / Chunk / Bytes 三条解码路径并断言一致。
fn assert_all_decode_paths(data: &[TestData]) {
    let (ids, datums) = fixture_inputs(data, false);
    let encoded = rowcodec::Encoder::new(true)
        .Encode(Some(&time::UTC), ids, datums, None, Vec::new())
        .expect("encode fixture");
    let mut decoder =
        rowcodec::NewDatumMapDecoder(data.iter().map(col_info).collect(), Some(time::UTC));
    let map = decoder
        .DecodeToDatumMap(&encoded, None)
        .expect("datum map decode");
    for item in data {
        assert_eq!(
            item.input.Kind(),
            map[&item.id].Kind(),
            "datum map column {}",
            item.id
        );
        assert_datum(&item.input, &map[&item.id]);
    }
    for (expected, actual) in data.iter().zip(decode_chunk(
        data,
        &encoded,
        vec![-1],
        Some(&kv::IntHandle(-1)),
        0,
        None,
    )) {
        assert_eq!(
            expected.input.Kind(),
            actual.Kind(),
            "chunk column {}",
            expected.id
        );
        assert_datum(&expected.input, &actual);
    }
    for (expected, actual) in data.iter().zip(decode_old_row(
        data,
        &encoded,
        vec![-1],
        &kv::IntHandle(-1),
        None,
    )) {
        assert_eq!(
            expected.output.Kind(),
            actual.Kind(),
            "old row column {}",
            expected.id
        );
        assert_datum(&expected.output, &actual);
    }
}

/// 新格式多类型编解码主路径。
#[test]
fn TestTypesNewRowCodec() {
    let small = typed_fixture();
    assert_all_decode_paths(&small);

    let mut large_column_id = typed_fixture();
    large_column_id[0].id = 300;
    assert_all_decode_paths(&large_column_id);

    let mut large_data = typed_fixture();
    let text = "a".repeat(u16::MAX as usize + 1);
    large_data[3].input = types::NewStringDatum(text.clone());
    large_data[3].output = types::NewBytesDatum(text.into_bytes());
    assert_all_decode_paths(&large_data);
}

/// NULL 列与默认值回调在解码时的填充语义。
#[test]
fn TestNilAndDefault() {
    let data = vec![
        TestData {
            id: 1,
            ft: field_type(mysql::TypeLonglong),
            input: types::NewIntDatum(1),
            output: types::NewIntDatum(1),
            default: None,
            handle: false,
        },
        TestData {
            id: 2,
            ft: unsigned(field_type(mysql::TypeLonglong)),
            input: types::NewUintDatum(1),
            output: types::NewUintDatum(9),
            default: Some(types::NewUintDatum(9)),
            handle: false,
        },
    ];
    let (ids, datums) = fixture_inputs(&data, true);
    let encoded = rowcodec::Encoder::new(true)
        .Encode(Some(&time::UTC), ids, datums, None, Vec::new())
        .expect("encode defaults");
    let mut map_decoder =
        rowcodec::NewDatumMapDecoder(data.iter().map(col_info).collect(), Some(time::UTC));
    let map = map_decoder
        .DecodeToDatumMap(&encoded, None)
        .expect("map decode");
    assert!(map.contains_key(&1));
    assert!(!map.contains_key(&2));

    let chunk_default = data[1].output.clone();
    let callback = Box::new(move |index: usize, chunk: &mut chunk::Chunk| {
        if index == 1 {
            chunk.AppendDatum(index, &chunk_default);
        } else {
            chunk.AppendNull(index);
        }
        Ok(())
    });
    let decoded = decode_chunk(
        &data,
        &encoded,
        vec![-1],
        Some(&kv::IntHandle(-1)),
        0,
        Some(callback),
    );
    assert_datum(&data[0].output, &decoded[0]);
    assert_datum(&data[1].output, &decoded[1]);

    let without_default =
        decode_chunk(&data, &encoded, vec![-1], Some(&kv::IntHandle(-1)), 0, None);
    assert!(without_default[1].IsNull());

    let default_bytes =
        tablecodec::EncodeValue(None, Vec::new(), data[1].output.clone()).expect("default bytes");
    let byte_callback = Box::new(move |index: usize| {
        Ok(if index == 1 {
            default_bytes.clone()
        } else {
            Vec::new()
        })
    });
    let old = decode_old_row(
        &data,
        &encoded,
        vec![-1],
        &kv::IntHandle(-1),
        Some(byte_callback),
    );
    assert_datum(&data[0].output, &old[0]);
    assert_datum(&data[1].output, &old[1]);
}

/// 新旧 integer varint 编码兼容性。
#[test]
fn TestVarintCompatibility() {
    let data = vec![
        TestData {
            id: 1,
            ft: field_type(mysql::TypeLonglong),
            input: types::NewIntDatum(1),
            output: types::NewIntDatum(1),
            default: None,
            handle: false,
        },
        TestData {
            id: 2,
            ft: unsigned(field_type(mysql::TypeLonglong)),
            input: types::NewUintDatum(1),
            output: types::NewUintDatum(1),
            default: None,
            handle: false,
        },
    ];
    let (ids, datums) = fixture_inputs(&data, false);
    let encoded = rowcodec::Encoder::new(true)
        .Encode(Some(&time::UTC), ids, datums, None, Vec::new())
        .expect("encode");
    let offsets = HashMap::from([(1, 0), (2, 1)]);
    let decoder = rowcodec::NewByteDecoder(
        data.iter().map(col_info).collect(),
        vec![-1],
        None,
        Some(time::UTC),
    );
    let old = decoder
        .DecodeToBytes(&offsets, &kv::IntHandle(1), &encoded, &[])
        .expect("decode bytes");
    for (index, item) in data.iter().enumerate() {
        assert_eq!(
            old[index],
            tablecodec::EncodeValue(None, Vec::new(), item.output.clone()).expect("old encoding")
        );
    }
}

/// 构造旧格式行字节夹具（列 ID + 字段类型 + 编码数据）。
fn old_row_fixture() -> (Vec<i64>, Vec<types::FieldType>, Vec<u8>) {
    let ids = vec![1, 2, 3, 4];
    let fields = vec![
        field_type(mysql::TypeLonglong),
        field_type(mysql::TypeLonglong),
        field_type(mysql::TypeLonglong),
        field_type(mysql::TypeNull),
    ];
    let row = vec![
        types::NewIntDatum(1),
        types::NewIntDatum(2),
        types::NewIntDatum(3),
        types::Datum::default(),
    ];
    let encoded = tablecodec::EncodeOldRow(
        codec::NewEncoder(collate::NewCollationEnabled()),
        Some(time::UTC),
        row,
        ids.clone(),
        Vec::new(),
        None,
    )
    .expect("old row encode");
    (ids, fields, encoded)
}

/// Codec 工具函数（如 ColumnIsNull）行为校验。
#[test]
fn TestCodecUtil() {
    let (ids, fields, old) = old_row_fixture();
    let new = encode_from_old_row(&mut rowcodec::Encoder::new(true), None, &old, Vec::new())
        .expect("convert old row");
    assert!(rowcodec::IsNewFormat(&new));
    assert!(!rowcodec::IsNewFormat(&old));
    assert!(!rowcodec::IsNewFormat(&[]));

    let columns = ids
        .iter()
        .zip(&fields)
        .map(|(id, ft)| rowcodec::ColInfo {
            ID: *id,
            IsPKHandle: false,
            VirtualGenCol: false,
            Ft: ft.clone(),
        })
        .collect();
    let mut decoder = rowcodec::NewDecoder(columns, vec![-1], None);
    assert!(decoder.ColumnIsNull(&new, 4, None).expect("null column"));
    assert!(
        !decoder
            .ColumnIsNull(&new, 1, None)
            .expect("non-null column")
    );
    assert!(decoder.ColumnIsNull(&new, 5, None).expect("missing column"));
    assert!(
        !decoder
            .ColumnIsNull(&new, 5, Some(&[1]))
            .expect("default column")
    );
    assert!(!rowcodec::IsRowKey(&[b'b', b't']));
    assert!(!rowcodec::IsRowKey(&[b't', b'r']));
}

/// 旧格式行经 `EncodeFromOldRow` 转新格式后再解码。
#[test]
fn TestOldRowCodec() {
    let (ids, fields, old) = old_row_fixture();
    let new = encode_from_old_row(&mut rowcodec::Encoder::new(true), None, &old, Vec::new())
        .expect("convert old row");
    let data: Vec<_> = ids
        .iter()
        .zip(&fields)
        .map(|(id, ft)| TestData {
            id: *id,
            ft: ft.clone(),
            input: types::Datum::default(),
            output: types::Datum::default(),
            default: None,
            handle: false,
        })
        .collect();
    let decoded = decode_chunk(&data, &new, vec![-1], Some(&kv::IntHandle(-1)), 0, None);
    for (index, datum) in decoded.iter().take(3).enumerate() {
        assert_eq!((index + 1) as i64, datum.GetInt64());
    }
}

/// 数据区长度触及 u16 上限时的 large 升级边界（65535 bug）。
#[test]
fn Test65535Bug() {
    let text = "a".repeat(65_535);
    let encoded = rowcodec::Encoder::new(true)
        .Encode(
            Some(&time::UTC),
            vec![1],
            vec![types::NewStringDatum(text.clone())],
            None,
            Vec::new(),
        )
        .expect("encode 65535-byte string");
    let mut decoder = rowcodec::NewDatumMapDecoder(
        vec![rowcodec::ColInfo {
            ID: 1,
            Ft: field_type(mysql::TypeString),
            IsPKHandle: false,
            VirtualGenCol: false,
        }],
        None,
    );
    let decoded = decoder
        .DecodeToDatumMap(&encoded, None)
        .expect("decode 65535-byte string");
    assert_eq!(text, decoded[&1].GetString());
}

/// 单列编码用例：类型、输入字节与期望 Datum。
struct ColumnCase {
    name: String,
    field_type: types::FieldType,
    datum: types::Datum,
    expected: Vec<u8>,
    succeeds: bool,
}

/// 小端 u64 字节序列夹具。
fn little_u64(value: u64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

/// 带长度前缀的字节序列夹具。
fn length_bytes(value: &[u8]) -> Vec<u8> {
    let mut result = (value.len() as u32).to_le_bytes().to_vec();
    result.extend_from_slice(value);
    result
}

/// 由 CoreTime 构造指定类型与 fsp 的 Time。
fn time_for(core: types::CoreTime, tp: u8, fsp: i32) -> types::Time {
    types::NewTime(core, tp, fsp)
}

/// 各列类型单独编码/解码正确性矩阵。
#[test]
fn TestColumnEncode() {
    let mut cases = vec![
        ColumnCase {
            name: "unspecified".into(),
            field_type: field_type(mysql::TypeUnspecified),
            datum: types::NewIntDatum(1),
            expected: vec![],
            succeeds: false,
        },
        ColumnCase {
            name: "wrong".into(),
            field_type: field_type(42),
            datum: types::NewIntDatum(1),
            expected: vec![],
            succeeds: false,
        },
    ];
    for (name, tp) in [
        ("mismatch/timestamp", mysql::TypeTimestamp),
        ("mismatch/datetime", mysql::TypeDatetime),
        ("mismatch/date", mysql::TypeDate),
        ("mismatch/newdate", mysql::TypeNewDate),
        ("mismatch/decimal", mysql::TypeNewDecimal),
    ] {
        cases.push(ColumnCase {
            name: name.into(),
            field_type: field_type(tp),
            datum: types::NewIntDatum(1),
            expected: vec![],
            succeeds: false,
        });
    }
    cases.push(ColumnCase {
        name: "null".into(),
        field_type: field_type(mysql::TypeNull),
        datum: types::NewIntDatum(1),
        expected: vec![],
        succeeds: true,
    });
    cases.push(ColumnCase {
        name: "geometry".into(),
        field_type: field_type(mysql::TypeGeometry),
        datum: types::NewIntDatum(1),
        expected: vec![],
        succeeds: true,
    });

    let integer_types = [
        (
            "tinyint",
            mysql::TypeTiny,
            i8::MIN as i64,
            i8::MAX as u64,
            u8::MAX as u64,
        ),
        (
            "smallint",
            mysql::TypeShort,
            i16::MIN as i64,
            i16::MAX as u64,
            u16::MAX as u64,
        ),
        (
            "int",
            mysql::TypeLong,
            i32::MIN as i64,
            i32::MAX as u64,
            u32::MAX as u64,
        ),
        (
            "bigint",
            mysql::TypeLonglong,
            i64::MIN,
            i64::MAX as u64,
            u64::MAX,
        ),
        (
            "mediumint",
            mysql::TypeInt24,
            -(1 << 23),
            (1 << 23) - 1,
            (1 << 24) - 1,
        ),
    ];
    for (name, tp, minimum, signed_maximum, unsigned_maximum) in integer_types {
        for (suffix, value) in [
            ("zero", 0_i64),
            ("pos", 42),
            ("neg", -2),
            ("min/signed", minimum),
        ] {
            cases.push(ColumnCase {
                name: format!("{name}/{suffix}"),
                field_type: field_type(tp),
                datum: types::NewIntDatum(value),
                expected: little_u64(value as u64),
                succeeds: true,
            });
        }
        cases.push(ColumnCase {
            name: format!("{name}/max/signed"),
            field_type: field_type(tp),
            datum: types::NewIntDatum(signed_maximum as i64),
            expected: little_u64(signed_maximum),
            succeeds: true,
        });
        cases.push(ColumnCase {
            name: format!("{name}/max/unsigned"),
            field_type: field_type(tp),
            datum: types::NewUintDatum(unsigned_maximum),
            expected: little_u64(unsigned_maximum),
            succeeds: true,
        });
    }
    cases.push(ColumnCase {
        name: "year".into(),
        field_type: field_type(mysql::TypeYear),
        datum: types::NewIntDatum(2023),
        expected: little_u64(2023),
        succeeds: true,
    });

    for (name, tp) in [
        ("varchar", mysql::TypeVarchar),
        ("varbinary", mysql::TypeVarString),
        ("char", mysql::TypeString),
        ("binary", mysql::TypeString),
        ("text", mysql::TypeBlob),
        ("blob", mysql::TypeBlob),
        ("longtext", mysql::TypeLongBlob),
        ("longblob", mysql::TypeLongBlob),
        ("mediumtext", mysql::TypeMediumBlob),
        ("mediumblob", mysql::TypeMediumBlob),
        ("tinytext", mysql::TypeTinyBlob),
        ("tinyblob", mysql::TypeTinyBlob),
    ] {
        let bytes_datum = name.contains("binary") || name.contains("blob");
        for (suffix, value) in [("", b"foo".as_slice()), ("/empty", b"".as_slice())] {
            let datum = if bytes_datum {
                types::NewBytesDatum(value.to_vec())
            } else {
                types::NewStringDatum(String::from_utf8(value.to_vec()).unwrap())
            };
            cases.push(ColumnCase {
                name: format!("{name}{suffix}"),
                field_type: field_type(tp),
                datum,
                expected: length_bytes(value),
                succeeds: true,
            });
        }
    }

    for (name, datum, bits) in [
        (
            "float",
            types::NewFloat32Datum(3.14),
            f64::from(3.14_f32).to_bits(),
        ),
        (
            "float/nan",
            types::NewFloat32Datum(f32::NAN),
            0_f64.to_bits(),
        ),
        (
            "float/+inf",
            types::NewFloat32Datum(f32::INFINITY),
            0_f64.to_bits(),
        ),
        (
            "float/-inf",
            types::NewFloat32Datum(f32::NEG_INFINITY),
            0_f64.to_bits(),
        ),
    ] {
        cases.push(ColumnCase {
            name: name.into(),
            field_type: field_type(mysql::TypeFloat),
            datum,
            expected: little_u64(bits),
            succeeds: true,
        });
    }
    for (name, value, bits) in [
        ("double", 3.14_f64, 3.14_f64.to_bits()),
        ("double/nan", f64::NAN, 0_f64.to_bits()),
        ("double/+inf", f64::INFINITY, 0_f64.to_bits()),
        ("double/-inf", f64::NEG_INFINITY, 0_f64.to_bits()),
    ] {
        cases.push(ColumnCase {
            name: name.into(),
            field_type: field_type(mysql::TypeDouble),
            datum: types::NewFloat64Datum(value),
            expected: little_u64(bits),
            succeeds: true,
        });
    }
    cases.push(ColumnCase {
        name: "enum".into(),
        field_type: field_type(mysql::TypeEnum),
        datum: types::NewUintDatum(0b010),
        expected: little_u64(0b010),
        succeeds: true,
    });
    cases.push(ColumnCase {
        name: "set".into(),
        field_type: field_type(mysql::TypeSet),
        datum: types::NewUintDatum(0b101),
        expected: little_u64(0b101),
        succeeds: true,
    });
    cases.push(ColumnCase {
        name: "bit".into(),
        field_type: field_type(mysql::TypeBit),
        datum: types::NewBinaryLiteralDatum(types::BinaryLiteral(vec![0x12, 0x34])),
        expected: little_u64(0x1234),
        succeeds: true,
    });
    cases.push(ColumnCase {
        name: "bit/truncate".into(),
        field_type: field_type(mysql::TypeBit),
        datum: types::NewBinaryLiteralDatum(types::BinaryLiteral(vec![
            0x12, 0x34, 0x12, 0x34, 0x12, 0x34, 0x12, 0x34, 0xff,
        ])),
        expected: little_u64(u64::MAX),
        succeeds: true,
    });

    let local = chrono_tz::Asia::Shanghai;
    let core = types::FromDate(2023, 1, 2, 3, 4, 5, 678);
    let zero = types::CoreTime(0);
    let mut timestamp = time_for(core, mysql::TypeTimestamp, 3);
    timestamp
        .ConvertTimeZone(local, time::UTC)
        .expect("convert timestamp");
    let temporal = [
        (
            "timestamp",
            mysql::TypeTimestamp,
            time_for(core, mysql::TypeTimestamp, 3),
            timestamp.String(),
        ),
        (
            "timestamp/zero",
            mysql::TypeTimestamp,
            time_for(zero, mysql::TypeTimestamp, 0),
            time_for(zero, mysql::TypeTimestamp, 0).String(),
        ),
        (
            "timestamp/min",
            mysql::TypeTimestamp,
            types::MinTimestamp(),
            {
                let mut value = types::MinTimestamp();
                value.ConvertTimeZone(local, time::UTC).unwrap();
                value.String()
            },
        ),
        (
            "timestamp/max",
            mysql::TypeTimestamp,
            types::MaxTimestamp(),
            {
                let mut value = types::MaxTimestamp();
                value.ConvertTimeZone(local, time::UTC).unwrap();
                value.String()
            },
        ),
        (
            "datetime",
            mysql::TypeDatetime,
            time_for(core, mysql::TypeDatetime, 3),
            time_for(core, mysql::TypeDatetime, 3).String(),
        ),
        (
            "datetime/zero",
            mysql::TypeDatetime,
            time_for(zero, mysql::TypeDatetime, 0),
            time_for(zero, mysql::TypeDatetime, 0).String(),
        ),
        (
            "datetime/min",
            mysql::TypeDatetime,
            types::MinDatetime(),
            types::MinDatetime().String(),
        ),
        (
            "datetime/max",
            mysql::TypeDatetime,
            types::MaxDatetime(),
            types::MaxDatetime().String(),
        ),
        (
            "date",
            mysql::TypeDate,
            time_for(core, mysql::TypeDate, 0),
            time_for(core, mysql::TypeDate, 0).String(),
        ),
        (
            "date/zero",
            mysql::TypeDate,
            time_for(zero, mysql::TypeDate, 0),
            time_for(zero, mysql::TypeDate, 0).String(),
        ),
        (
            "date/min",
            mysql::TypeDate,
            time_for(types::MinDatetime().CoreTime(), mysql::TypeDate, 0),
            time_for(types::MinDatetime().CoreTime(), mysql::TypeDate, 0).String(),
        ),
        (
            "date/max",
            mysql::TypeDate,
            time_for(types::MaxDatetime().CoreTime(), mysql::TypeDate, 0),
            time_for(types::MaxDatetime().CoreTime(), mysql::TypeDate, 0).String(),
        ),
        (
            "newdate",
            mysql::TypeNewDate,
            time_for(core, mysql::TypeNewDate, 0),
            time_for(core, mysql::TypeNewDate, 0).String(),
        ),
        (
            "newdate/zero",
            mysql::TypeNewDate,
            time_for(zero, mysql::TypeNewDate, 0),
            time_for(zero, mysql::TypeNewDate, 0).String(),
        ),
        (
            "newdate/min",
            mysql::TypeNewDate,
            time_for(types::MinDatetime().CoreTime(), mysql::TypeNewDate, 0),
            time_for(types::MinDatetime().CoreTime(), mysql::TypeNewDate, 0).String(),
        ),
        (
            "newdate/max",
            mysql::TypeNewDate,
            time_for(types::MaxDatetime().CoreTime(), mysql::TypeNewDate, 0),
            time_for(types::MaxDatetime().CoreTime(), mysql::TypeNewDate, 0).String(),
        ),
    ];
    for (name, tp, value, expected) in temporal {
        cases.push(ColumnCase {
            name: name.into(),
            field_type: field_type(tp),
            datum: types::NewTimeDatum(value),
            expected: length_bytes(expected.as_bytes()),
            succeeds: true,
        });
    }

    let duration = types::Duration {
        Duration: 8 * 60 * 60 * 1_000_000_000 + 7 * 60 * 1_000_000_000 + 123_456 * 1_000,
        Fsp: 6,
    };
    for (name, value) in [
        ("time", duration),
        ("time/zero", types::ZeroDuration),
        (
            "time/max",
            types::Duration {
                Duration: types::MaxDuration,
                Fsp: 3,
            },
        ),
    ] {
        cases.push(ColumnCase {
            name: name.into(),
            field_type: field_type(mysql::TypeDuration),
            datum: types::NewDurationDatum(value),
            expected: length_bytes(value.String().as_bytes()),
            succeeds: true,
        });
    }
    for (name, value) in [
        ("decimal/zero", decimal("0.000")),
        ("decimal/pos", decimal("3.14")),
        ("decimal/neg", decimal("-1.2")),
        ("decimal/min", types::NewMaxOrMinDec(true, 12, 6)),
        ("decimal/max", types::NewMaxOrMinDec(false, 12, 6)),
    ] {
        cases.push(ColumnCase {
            name: name.into(),
            field_type: field_type(mysql::TypeNewDecimal),
            datum: types::NewDecimalDatum(value.clone()),
            expected: length_bytes(value.String().as_bytes()),
            succeeds: true,
        });
    }
    for (name, json) in [
        ("json/1", types::ParseBinaryJSONFromString("null").unwrap()),
        ("json/2", types::ParseBinaryJSONFromString("42").unwrap()),
        (
            "json/3",
            types::ParseBinaryJSONFromString(r#"{"foo":"bar","a":42}"#).unwrap(),
        ),
    ] {
        cases.push(ColumnCase {
            name: name.into(),
            field_type: field_type(mysql::TypeJSON),
            datum: types::NewJSONDatum(json.clone()),
            expected: length_bytes(json.String().as_bytes()),
            succeeds: true,
        });
    }

    for case in cases {
        let column = model::ColumnInfo {
            FieldType: case.field_type,
            ..Default::default()
        };
        let data = rowcodec::ColData {
            ColumnInfo: &column,
            Datum: &case.datum,
        };
        let encoded = data.Encode(Some(&local), Vec::new());
        if case.succeeds {
            assert_eq!(
                case.expected,
                encoded.unwrap_or_else(|error| panic!("{}: {error}", case.name)),
                "{}",
                case.name
            );
        } else {
            assert!(encoded.is_err(), "{} unexpectedly succeeded", case.name);
        }
    }

    for tp in [
        mysql::TypeUnspecified,
        mysql::TypeTiny,
        mysql::TypeShort,
        mysql::TypeLong,
        mysql::TypeFloat,
        mysql::TypeDouble,
        mysql::TypeNull,
        mysql::TypeTimestamp,
        mysql::TypeLonglong,
        mysql::TypeInt24,
        mysql::TypeDate,
        mysql::TypeDuration,
        mysql::TypeDatetime,
        mysql::TypeYear,
        mysql::TypeNewDate,
        mysql::TypeVarchar,
        mysql::TypeBit,
        mysql::TypeJSON,
        mysql::TypeNewDecimal,
        mysql::TypeEnum,
        mysql::TypeSet,
        mysql::TypeTinyBlob,
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        mysql::TypeBlob,
        mysql::TypeVarString,
        mysql::TypeString,
        mysql::TypeGeometry,
        42,
    ] {
        let column = model::ColumnInfo {
            FieldType: field_type(tp),
            ..Default::default()
        };
        let datum = types::Datum::default();
        let data = rowcodec::ColData {
            ColumnInfo: &column,
            Datum: &datum,
        };
        assert!(
            data.Encode(Some(&local), Vec::new())
                .expect("null datum encodes")
                .is_empty()
        );
    }
}

/// 行级 checksum 读写与版本字段。
#[test]
fn TestRowChecksum() {
    let columns = [
        model::ColumnInfo {
            ID: 1,
            FieldType: field_type(mysql::TypeNull),
            ..Default::default()
        },
        model::ColumnInfo {
            ID: 2,
            FieldType: field_type(mysql::TypeLong),
            ..Default::default()
        },
        model::ColumnInfo {
            ID: 3,
            FieldType: field_type(mysql::TypeVarchar),
            ..Default::default()
        },
        model::ColumnInfo {
            ID: 4,
            FieldType: field_type(mysql::TypeTimestamp),
            ..Default::default()
        },
    ];
    let datums = [
        types::Datum::default(),
        types::NewIntDatum(42),
        types::NewStringDatum("foobar".to_owned()),
        types::NewTimeDatum(time_for(
            types::FromDate(2026, 7, 14, 12, 0, 0, 0),
            mysql::TypeTimestamp,
            6,
        )),
    ];
    for (name, indexes) in [
        ("nil", vec![]),
        ("empty", vec![]),
        ("nullonly", vec![0]),
        ("ordered", vec![0, 1, 2, 3]),
        ("unordered", vec![2, 0, 3, 1]),
    ] {
        let cols = indexes
            .into_iter()
            .map(|index| rowcodec::ColData {
                ColumnInfo: &columns[index],
                Datum: &datums[index],
            })
            .collect();
        let mut row = rowcodec::RowData {
            Cols: cols,
            Data: Vec::with_capacity(64),
        };
        for left in 0..row.Len() {
            for right in left + 1..row.Len() {
                if row.Less(right, left) {
                    row.Swap(left, right);
                }
            }
        }
        let checksum = row
            .Checksum(Some(&time::UTC))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let raw = row.Encode(Some(&time::UTC)).expect("row encode");
        assert_eq!(crc32fast::hash(&raw), checksum, "{name}");
    }
}

/// 带 checksum 的完整编解码往返。
#[test]
fn TestEncodeDecodeRowWithChecksum() {
    let mut encoder = rowcodec::Encoder::new(true);
    let raw = encoder
        .Encode(Some(&time::UTC), vec![], vec![], None, Vec::new())
        .expect("empty row");
    let mut decoder = rowcodec::NewDatumMapDecoder(vec![], Some(time::UTC));
    decoder
        .DecodeToDatumMap(&raw, None)
        .expect("decode empty row");
    assert_eq!((0, false), decoder.GetChecksum());

    let raw = encoder
        .Encode(
            Some(&time::UTC),
            vec![],
            vec![],
            Some(Box::new(rowcodec::RawChecksum {
                Handle: Box::new(kv::IntHandle(1)),
            })),
            Vec::new(),
        )
        .expect("checksum row");
    let (expected, ok) = encoder.GetChecksum();
    assert!(ok);
    assert_ne!(0, expected);
    decoder
        .DecodeToDatumMap(&raw, None)
        .expect("decode checksum row");
    assert_eq!((expected, true), decoder.GetChecksum());
    assert_eq!(2, decoder.ChecksumVersion());
}

/// ExtraCommitTSID 特殊列从 commitTS 参数填充。
#[test]
fn TestDecodeWithCommitTS() {
    let mut commit_type = field_type(mysql::TypeLonglong);
    commit_type.SetFlag(mysql::UnsignedFlag);
    let data = vec![
        TestData {
            id: 1,
            ft: field_type(mysql::TypeString),
            input: types::NewStringDatum("test1".to_owned()),
            output: types::NewStringDatum("test1".to_owned()),
            default: None,
            handle: false,
        },
        TestData {
            id: model::ExtraCommitTSID,
            ft: commit_type,
            input: types::Datum::default(),
            output: types::NewUintDatum(123_456),
            default: None,
            handle: false,
        },
        TestData {
            id: 2,
            ft: field_type(mysql::TypeString),
            input: types::NewStringDatum("test2".to_owned()),
            output: types::NewStringDatum("test2".to_owned()),
            default: None,
            handle: false,
        },
    ];
    let encoded = rowcodec::Encoder::new(true)
        .Encode(
            Some(&time::UTC),
            vec![1, 2],
            vec![data[0].input.clone(), data[2].input.clone()],
            None,
            Vec::new(),
        )
        .expect("encode commit-ts row");
    let decoded = decode_chunk(&data, &encoded, vec![-1], None, 123_456, None);
    assert_eq!("test1", decoded[0].GetString());
    assert_eq!(123_456, decoded[1].GetUint64());
    assert_eq!("test2", decoded[2].GetString());
}
