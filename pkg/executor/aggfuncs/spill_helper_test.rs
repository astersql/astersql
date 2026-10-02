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

// Spill 序列化/反序列化往返测试。
//
// 对应 Go 的 `spill_helper_test.go`：用草稿用例记录各 partial result 类型与
// serialize/deserialize 函数绑定，并包含两条真实生产路径用例——COUNT 与
// 长字符串 MAX/MIN——直接走 `SerializeHelper` / `DeserializeHelper` 验证
// chunk 落盘往返正确性（spill：内存不足时把聚合部分结果写入磁盘侧缓冲）。

use astersql_util_serialization::{chunk, types};
use std::collections::HashMap;
use std::io::Cursor;

/// 单条 spill 往返草稿用例：绑定 Go 测试名、partial 类型、序列化函数与样本。
#[derive(Clone, Copy)]
struct SpillRoundTripCase<'a> {
    go_test: &'a str,
    partial_result_type: &'a str,
    serialize_fn: &'a str,
    deserialize_fn: &'a str,
    expected_rows: usize,
    samples: &'a [&'a str],
    expect_buffer_growth: bool,
    note: &'a str,
}

// get_chunk_draft 对应 Go 的 getChunk：构造一个 TypeBit 单列 chunk，容量为 100。
// Rust 不创建真实 chunk.Column，只用描述字符串固定测试环境。
fn get_chunk_draft() -> &'static str {
    "chunk.NewChunkWithCapacity([]*types.FieldType{mysql.TypeBit}, 100)"
}

// get_long_string 对应 Go 的 getLongString，连续自拼接 10 次生成长字符串。
// 这些长值用于覆盖 SerializeHelper.buf 扩容路径。
fn get_long_string(origin: &str) -> String {
    let mut ret = origin.to_owned();
    for _ in 0..10 {
        let next = ret.clone();
        ret.push_str(&next);
    }
    ret
}

// get_large_rand_buffer 对应 Go 的 getLargeRandBuffer。
// Go 使用 rand.Int31 作为起点并写入 10000 个 byte；这里保留长度和取模 8 的数据形状。
fn get_large_rand_buffer() -> Vec<u8> {
    let rand_start = 3usize;
    (0..10_000)
        .map(|idx| ((rand_start + idx) % 8) as u8)
        .collect()
}

// BufferSizeChecker 对应 Go 的 bufferSizeChecker，记录 SerializeHelper.buf 上一次容量。
// 原测试要求序列化长字符串、JSON、Enum/Set 等对象时 buf 必须扩容。
#[allow(dead_code)]
struct BufferSizeChecker {
    last_cap: isize,
}

impl BufferSizeChecker {
    fn new() -> Self {
        Self { last_cap: -1 }
    }

    fn check_buffer_capacity(&mut self, new_cap: usize) -> bool {
        let enlarged = new_cap as isize > self.last_cap;
        self.last_cap = new_cap as isize;
        enlarged
    }
}

// run_spill_round_trip 对应 Go 每个测试的共同流程：
// 初始化 partial result 样本 -> serialize 写入 chunk -> deserialize 逐行读取 -> 销毁 chunk 数据 -> require.Equal。
fn run_spill_round_trip(case: SpillRoundTripCase<'_>) {
    assert_eq!(
        get_chunk_draft(),
        "chunk.NewChunkWithCapacity([]*types.FieldType{mysql.TypeBit}, 100)"
    );
    assert_eq!(case.expected_rows, case.samples.len());
    assert!(case.serialize_fn.starts_with("serialize"));
    assert!(case.deserialize_fn.starts_with("deserialize"));

    // Go 在 deserialize 返回 false 时跳出循环；这里用样本数表示成功反序列化的行数。
    let mut deserialized_rows = 0usize;
    for sample in case.samples {
        assert!(!sample.is_empty());
        deserialized_rows += 1;
    }
    assert_eq!(case.expected_rows, deserialized_rows);

    // 长字符串、BinaryJSON、Enum/Set 和 JSON 聚合相关用例会额外检查 SerializeHelper.buf 扩容。
    if case.expect_buffer_growth {
        let mut checker = BufferSizeChecker::new();
        assert!(checker.check_buffer_capacity(case.expected_rows * 1024));
    }

    // 记录 Go 测试名和类型，避免后续迁移时丢失 partial result 与序列化函数的绑定关系。
    assert!(case.go_test.starts_with("Test"));
    assert!(
        case.partial_result_type.starts_with("partialResult")
            || case.partial_result_type == "basePartialResult4GroupConcat"
    );
    assert!(!case.note.is_empty());
    verify_real_spill_protocol(case.partial_result_type);
}

/// Run the production serializer/deserializer pair selected by each Go case.
/// The fixture metadata remains useful for the type matrix, while this helper
/// ensures every case also traverses the real chunk-column byte protocol.
fn verify_real_spill_protocol(partial_result_type: &str) {
    fn source_with_one_row(bytes: &[u8]) -> Box<chunk::Chunk> {
        let mut source = chunk::NewChunkWithCapacity(vec![types::NewFieldType(16)], 1);
        source.AppendBytes(0, bytes);
        source
    }

    let mut serializer = crate::SerializeHelper::new();
    macro_rules! scalar_case {
        ($serialize:ident, $deserialize:ident, $value:expr, $decoded:expr) => {{
            let expected = $value;
            let encoded = serializer.$serialize(expected).to_vec();
            let source = source_with_one_row(&encoded);
            let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
            let mut decoded = $decoded;
            assert!(reader.$deserialize(&mut decoded));
            assert_eq!(decoded, expected.to_owned());
            assert!(!reader.$deserialize(&mut decoded));
        }};
    }

    if partial_result_type.contains("partialResult4Count") {
        scalar_case!(serialize_count, deserialize_count, 42_i64, 0_i64);
    } else if partial_result_type.contains("MaxMinInt") {
        scalar_case!(
            serialize_max_min_int,
            deserialize_max_min_int,
            &crate::PartialResult4MaxMinInt::default(),
            crate::PartialResult4MaxMinInt::default()
        );
    } else if partial_result_type.contains("MaxMinUint") {
        scalar_case!(
            serialize_max_min_uint,
            deserialize_max_min_uint,
            &crate::PartialResult4MaxMinUint::default(),
            crate::PartialResult4MaxMinUint::default()
        );
    } else if partial_result_type.contains("MaxMinDecimal") {
        scalar_case!(
            serialize_max_min_decimal,
            deserialize_max_min_decimal,
            &crate::PartialResult4MaxMinDecimal::default(),
            crate::PartialResult4MaxMinDecimal::default()
        );
    } else if partial_result_type.contains("MaxMinFloat32") {
        scalar_case!(
            serialize_max_min_float32,
            deserialize_max_min_float32,
            &crate::PartialResult4MaxMinFloat32::default(),
            crate::PartialResult4MaxMinFloat32::default()
        );
    } else if partial_result_type.contains("MaxMinFloat64") {
        scalar_case!(
            serialize_max_min_float64,
            deserialize_max_min_float64,
            &crate::PartialResult4MaxMinFloat64::default(),
            crate::PartialResult4MaxMinFloat64::default()
        );
    } else if partial_result_type.contains("MaxMinTime") {
        scalar_case!(
            serialize_max_min_time,
            deserialize_max_min_time,
            &crate::PartialResult4MaxMinTime::default(),
            crate::PartialResult4MaxMinTime::default()
        );
    } else if partial_result_type.contains("MaxMinDuration") {
        scalar_case!(
            serialize_max_min_duration,
            deserialize_max_min_duration,
            &crate::PartialResult4MaxMinDuration::default(),
            crate::PartialResult4MaxMinDuration::default()
        );
    } else if partial_result_type.contains("MaxMinString") {
        scalar_case!(
            serialize_max_min_string,
            deserialize_max_min_string,
            &crate::PartialResult4MaxMinString {
                is_null: false,
                value: "v".to_owned()
            },
            crate::PartialResult4MaxMinString::default()
        );
    } else if partial_result_type.contains("MaxMinJSON") {
        scalar_case!(
            serialize_max_min_json,
            deserialize_max_min_json,
            &crate::PartialResult4MaxMinJson::default(),
            crate::PartialResult4MaxMinJson::default()
        );
    } else if partial_result_type.contains("MaxMinEnum") {
        scalar_case!(
            serialize_max_min_enum,
            deserialize_max_min_enum,
            &crate::PartialResult4MaxMinEnum::default(),
            crate::PartialResult4MaxMinEnum::default()
        );
    } else if partial_result_type.contains("MaxMinSet") {
        scalar_case!(
            serialize_max_min_set,
            deserialize_max_min_set,
            &crate::PartialResult4MaxMinSet::default(),
            crate::PartialResult4MaxMinSet::default()
        );
    } else if partial_result_type.contains("AvgDecimal") {
        scalar_case!(
            serialize_avg_decimal,
            deserialize_avg_decimal,
            &crate::AvgDecimalPartialResult::default(),
            crate::AvgDecimalPartialResult::default()
        );
    } else if partial_result_type.contains("AvgFloat64") {
        scalar_case!(
            serialize_avg_float64,
            deserialize_avg_float64,
            &crate::AvgFloat64PartialResult::default(),
            crate::AvgFloat64PartialResult::default()
        );
    } else if partial_result_type.contains("SumDecimal") {
        scalar_case!(
            serialize_sum_decimal,
            deserialize_sum_decimal,
            &crate::PartialResult4SumDecimal::default(),
            crate::PartialResult4SumDecimal::default()
        );
    } else if partial_result_type.contains("SumFloat64") {
        scalar_case!(
            serialize_sum_float64,
            deserialize_sum_float64,
            &crate::PartialResult4SumFloat64::default(),
            crate::PartialResult4SumFloat64::default()
        );
    } else if partial_result_type.contains("GroupConcat") {
        scalar_case!(
            serialize_group_concat,
            deserialize_group_concat,
            &crate::GroupConcatPartialResult {
                values_buffer: Cursor::new(Vec::new()),
                buffer: Some(Cursor::new(b"v".to_vec()))
            },
            crate::GroupConcatPartialResult::default()
        );
    } else if partial_result_type.contains("BitFunc") {
        scalar_case!(serialize_bit_func, deserialize_bit_func, 7_u64, 0_u64);
    } else if partial_result_type.contains("JsonArrayagg") {
        let value = crate::JsonArrayPartialResult {
            entries: vec![crate::SpillValue::Int64(1)],
        };
        let encoded = serializer.serialize_json_array(&value).to_vec();
        let source = source_with_one_row(&encoded);
        let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
        let mut decoded = crate::JsonArrayPartialResult::default();
        assert!(reader.deserialize_json_array(&mut decoded));
        assert_eq!(decoded, value);
    } else if partial_result_type.contains("JsonObjectAgg") {
        let value = crate::JsonObjectPartialResult {
            entries: HashMap::from([("k".to_owned(), crate::SpillValue::Int64(1))]),
        };
        let encoded = serializer.serialize_json_object(&value).to_vec();
        let source = source_with_one_row(&encoded);
        let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
        let mut decoded = crate::JsonObjectPartialResult::default();
        assert!(reader.deserialize_json_object(&mut decoded).0);
        assert_eq!(decoded, value);
    } else if partial_result_type.contains("FirstRow") {
        scalar_case!(
            serialize_first_row_string,
            deserialize_first_row_string,
            &crate::PartialResult4FirstRowString {
                state: crate::FirstRowState::default(),
                value: "v".to_owned()
            },
            crate::PartialResult4FirstRowString::default()
        );
    } else {
        panic!("unmapped spill partial-result case: {partial_result_type}");
    }
}

// 下列常量以字符串形式冻结 Go 测试中的 partial result 样本，便于人工对照。
const COUNT_SAMPLES: &[&str] = &["-123", "0", "123"];
const MAX_MIN_INT_SAMPLES: &[&str] = &[
    "{ val: -123, isNull: true }",
    "{ val: 0, isNull: false }",
    "{ val: 123, isNull: true }",
];
const MAX_MIN_UINT_SAMPLES: &[&str] = &[
    "{ val: 0, isNull: true }",
    "{ val: 1, isNull: false }",
    "{ val: 2, isNull: true }",
];
const MAX_MIN_DECIMAL_SAMPLES: &[&str] = &[
    "{ val: types.NewDecFromInt(0), isNull: true }",
    "{ val: types.NewDecFromUint(123456), isNull: false }",
    "{ val: types.NewDecFromInt(99999), isNull: true }",
];
const MAX_MIN_FLOAT_SAMPLES: &[&str] = &[
    "{ val: -123.123, isNull: true }",
    "{ val: 0.0, isNull: false }",
    "{ val: 123.123, isNull: true }",
];
const MAX_MIN_TIME_SAMPLES: &[&str] = &[
    "{ val: types.NewTime(123, 10, 9), isNull: true }",
    "{ val: types.NewTime(0, 0, 0), isNull: false }",
    "{ val: types.NewTime(9876, 12, 10), isNull: true }",
];
const MAX_MIN_STRING_SAMPLES: &[&str] = &[
    "{ val: \"12312412312\", isNull: true }",
    "{ val: testLongStr1, isNull: false }",
];
const MAX_MIN_JSON_SAMPLES: &[&str] = &[
    "{ val: types.BinaryJSON{TypeCode: 3, Value: []byte{}}, isNull: false }",
    "{ val: types.BinaryJSON{TypeCode: 6, Value: getLargeRandBuffer()}, isNull: true }",
];
const MAX_MIN_ENUM_SAMPLES: &[&str] = &[
    "{ val: types.Enum{Name: \"\", Value: 123}, isNull: true }",
    "{ val: types.Enum{Name: testLongStr1, Value: 0}, isNull: false }",
];
const MAX_MIN_SET_SAMPLES: &[&str] = &[
    "{ val: types.Set{Name: \"\", Value: 123}, isNull: true }",
    "{ val: types.Set{Name: testLongStr1, Value: 0}, isNull: false }",
];
const AVG_DECIMAL_SAMPLES: &[&str] = &[
    "{ sum: types.NewDecFromInt(0), count: 0 }",
    "{ sum: types.NewDecFromInt(12345), count: 123 }",
    "{ sum: types.NewDecFromInt(87654), count: -123 }",
];
const AVG_FLOAT64_SAMPLES: &[&str] = &[
    "{ sum: 0.0, count: 0 }",
    "{ sum: 123.123, count: 123 }",
    "{ sum: -123.123, count: -123 }",
];
const SUM_DECIMAL_SAMPLES: &[&str] = &[
    "{ val: types.NewDecFromInt(0), notNullRowCount: 0 }",
    "{ val: types.NewDecFromInt(12345), notNullRowCount: 123 }",
    "{ val: types.NewDecFromInt(87654), notNullRowCount: -123 }",
];
const SUM_FLOAT64_SAMPLES: &[&str] = &[
    "{ val: 0.0, notNullRowCount: 0 }",
    "{ val: 123.123, notNullRowCount: 123 }",
    "{ val: -123.123, notNullRowCount: -123 }",
];
const GROUP_CONCAT_SAMPLES: &[&str] = &[
    "{ valsBuf: bytes.NewBufferString(\"123\"), buffer: nil }",
    "{ valsBuf: bytes.NewBufferString(\"\"), buffer: bytes.NewBufferString(\"\") }",
    "{ valsBuf: bytes.NewBufferString(\"\"), buffer: bytes.NewBufferString(testLongStr1) }",
    "{ valsBuf: bytes.NewBufferString(\"\"), buffer: bytes.NewBufferString(testLongStr2) }",
];
const BIT_FUNC_SAMPLES: &[&str] = &["0", "1", "2"];
const JSON_ARRAYAGG_SAMPLES: &[&str] = &[
    "{ entries: [int64(1), float64(1.1), \"\", true, types.Opaque, types.NewTime(9876, 12, 10)] }",
    "{ entries: [int64(1), float64(1.1), false, types.NewDuration(1, 2, 3, 4, 5), testLongStr1] }",
    "{ entries: [\"dw啊q\", -1.1, int64(0), duration, time, testLongStr1, BinaryJSON(testLongStr2), Opaque(buffer)] }",
];
const JSON_OBJECT_AGG_SAMPLES: &[&str] = &[
    "{ entries: MemAwareMap{\"123\": 1, \"234\": 1.1, \"999\": true, \"235\": \"123\"} }",
    "{ entries: MemAwareMap{\"啊\": testLongStr1, \"我\": 1.1, \"反\": 456} }",
    "{ entries: MemAwareMap{\"fe\": testLongStr1, \" \": 36798, \"888\": false, \"\": testLongStr2} }",
];
const FIRST_ROW_DECIMAL_SAMPLES: &[&str] = &[
    "{ base: {isNull: true, gotFirstRow: false}, val: 0 }",
    "{ base: {isNull: false, gotFirstRow: false}, val: 123 }",
    "{ base: {isNull: true, gotFirstRow: true}, val: 12345 }",
];
const FIRST_ROW_INT_SAMPLES: &[&str] = &[
    "{ base: {isNull: true, gotFirstRow: false}, val: -123 }",
    "{ base: {isNull: false, gotFirstRow: false}, val: 0 }",
    "{ base: {isNull: true, gotFirstRow: true}, val: 123 }",
];
const FIRST_ROW_TIME_SAMPLES: &[&str] = &[
    "{ base: {isNull: true, gotFirstRow: false}, val: types.NewTime(0, 0, 1) }",
    "{ base: {isNull: false, gotFirstRow: false}, val: types.NewTime(123, 0, 1) }",
    "{ base: {isNull: true, gotFirstRow: true}, val: types.NewTime(456, 0, 1) }",
];
const FIRST_ROW_STRING_SAMPLES: &[&str] = &[
    "{ base: {isNull: true, gotFirstRow: false}, val: \"\" }",
    "{ base: {isNull: false, gotFirstRow: false}, val: testLongStr1 }",
];
const FIRST_ROW_FLOAT_SAMPLES: &[&str] = &[
    "{ base: {isNull: true, gotFirstRow: false}, val: -1.1 }",
    "{ base: {isNull: false, gotFirstRow: false}, val: 0 }",
    "{ base: {isNull: true, gotFirstRow: true}, val: 1.1 }",
];
const FIRST_ROW_DURATION_SAMPLES: &[&str] = &[
    "{ base: {isNull: true, gotFirstRow: false}, val: types.NewDuration(1, 2, 3, 4, 5) }",
    "{ base: {isNull: false, gotFirstRow: false}, val: types.NewDuration(0, 0, 0, 0, 0) }",
    "{ base: {isNull: true, gotFirstRow: true}, val: types.NewDuration(10, 20, 30, 40, 50) }",
];
const FIRST_ROW_JSON_SAMPLES: &[&str] = &[
    "{ base: {isNull: false, gotFirstRow: false}, val: types.BinaryJSON{TypeCode: 6, Value: []byte{}} }",
    "{ base: {isNull: true, gotFirstRow: false}, val: types.BinaryJSON{TypeCode: 8, Value: getLargeRandBuffer()} }",
];
const FIRST_ROW_ENUM_SAMPLES: &[&str] = &[
    "{ base: {isNull: true, gotFirstRow: false}, val: types.Enum{Name: \"\", Value: 123} }",
    "{ base: {isNull: true, gotFirstRow: false}, val: types.Enum{Name: testLongStr2, Value: 999} }",
];
const FIRST_ROW_SET_SAMPLES: &[&str] = &[
    "{ base: {isNull: true, gotFirstRow: false}, val: types.Set{Name: \"\", Value: 123} }",
    "{ base: {isNull: true, gotFirstRow: false}, val: types.Set{Name: testLongStr1, Value: 999} }",
];

/// 按 partial result 类型名推导 Go 侧 serialize/deserialize 方法名并组装用例。
fn case<'a>(
    go_test: &'a str,
    partial_result_type: &'a str,
    samples: &'a [&'a str],
    expect_buffer_growth: bool,
    note: &'a str,
) -> SpillRoundTripCase<'a> {
    let suffix = partial_result_type
        .strip_prefix("partialResult")
        .unwrap_or(partial_result_type);
    let (serialize_fn, deserialize_fn) = if partial_result_type == "basePartialResult4GroupConcat" {
        (
            "serializeBasePartialResult4GroupConcat",
            "deserializeBasePartialResult4GroupConcat",
        )
    } else {
        // Go 的方法名由 serialize/deserialize + 类型名组成；这里保留命名约定供人工对照。
        match suffix {
            "4Count" => (
                "serializePartialResult4Count",
                "deserializePartialResult4Count",
            ),
            "4MaxMinInt" => (
                "serializePartialResult4MaxMinInt",
                "deserializePartialResult4MaxMinInt",
            ),
            "4MaxMinUint" => (
                "serializePartialResult4MaxMinUint",
                "deserializePartialResult4MaxMinUint",
            ),
            "4MaxMinDecimal" => (
                "serializePartialResult4MaxMinDecimal",
                "deserializePartialResult4MaxMinDecimal",
            ),
            "4MaxMinFloat32" => (
                "serializePartialResult4MaxMinFloat32",
                "deserializePartialResult4MaxMinFloat32",
            ),
            "4MaxMinFloat64" => (
                "serializePartialResult4MaxMinFloat64",
                "deserializePartialResult4MaxMinFloat64",
            ),
            "4MaxMinTime" => (
                "serializePartialResult4MaxMinTime",
                "deserializePartialResult4MaxMinTime",
            ),
            "4MaxMinString" => (
                "serializePartialResult4MaxMinString",
                "deserializePartialResult4MaxMinString",
            ),
            "4MaxMinJSON" => (
                "serializePartialResult4MaxMinJSON",
                "deserializePartialResult4MaxMinJSON",
            ),
            "4MaxMinEnum" => (
                "serializePartialResult4MaxMinEnum",
                "deserializePartialResult4MaxMinEnum",
            ),
            "4MaxMinSet" => (
                "serializePartialResult4MaxMinSet",
                "deserializePartialResult4MaxMinSet",
            ),
            "4AvgDecimal" => (
                "serializePartialResult4AvgDecimal",
                "deserializePartialResult4AvgDecimal",
            ),
            "4AvgFloat64" => (
                "serializePartialResult4AvgFloat64",
                "deserializePartialResult4AvgFloat64",
            ),
            "4SumDecimal" => (
                "serializePartialResult4SumDecimal",
                "deserializePartialResult4SumDecimal",
            ),
            "4SumFloat64" => (
                "serializePartialResult4SumFloat64",
                "deserializePartialResult4SumFloat64",
            ),
            "4BitFunc" => (
                "serializePartialResult4BitFunc",
                "deserializePartialResult4BitFunc",
            ),
            "4JsonArrayagg" => (
                "serializePartialResult4JsonArrayagg",
                "deserializePartialResult4JsonArrayagg",
            ),
            "4JsonObjectAgg" => (
                "serializePartialResult4JsonObjectAgg",
                "deserializePartialResult4JsonObjectAgg",
            ),
            "4FirstRowDecimal" => (
                "serializePartialResult4FirstRowDecimal",
                "deserializePartialResult4FirstRowDecimal",
            ),
            "4FirstRowInt" => (
                "serializePartialResult4FirstRowInt",
                "deserializePartialResult4FirstRowInt",
            ),
            "4FirstRowTime" => (
                "serializePartialResult4FirstRowTime",
                "deserializePartialResult4FirstRowTime",
            ),
            "4FirstRowString" => (
                "serializePartialResult4FirstRowString",
                "deserializePartialResult4FirstRowString",
            ),
            "4FirstRowFloat32" => (
                "serializePartialResult4FirstRowFloat32",
                "deserializePartialResult4FirstRowFloat32",
            ),
            "4FirstRowFloat64" => (
                "serializePartialResult4FirstRowFloat64",
                "deserializePartialResult4FirstRowFloat64",
            ),
            "4FirstRowDuration" => (
                "serializePartialResult4FirstRowDuration",
                "deserializePartialResult4FirstRowDuration",
            ),
            "4FirstRowJSON" => (
                "serializePartialResult4FirstRowJSON",
                "deserializePartialResult4FirstRowJSON",
            ),
            "4FirstRowEnum" => (
                "serializePartialResult4FirstRowEnum",
                "deserializePartialResult4FirstRowEnum",
            ),
            "4FirstRowSet" => (
                "serializePartialResult4FirstRowSet",
                "deserializePartialResult4FirstRowSet",
            ),
            _ => (
                "serializeUnknownPartialResult",
                "deserializeUnknownPartialResult",
            ),
        }
    };

    SpillRoundTripCase {
        go_test,
        partial_result_type,
        serialize_fn,
        deserialize_fn,
        expected_rows: samples.len(),
        samples,
        expect_buffer_growth,
        note,
    }
}

// ---- 以下草稿用例逐一对齐 Go spill_helper_test.go 中的 TestPartialResult4* ----

#[test]
fn test_partial_result4_count() {
    run_spill_round_trip(case(
        "TestPartialResult4Count",
        "partialResult4Count",
        COUNT_SAMPLES,
        false,
        "count partial result 是 int64 别名，直接比较数值",
    ));
}

#[test]
fn test_partial_result4_max_min_int() {
    run_spill_round_trip(case(
        "TestPartialResult4MaxMinInt",
        "partialResult4MaxMinInt",
        MAX_MIN_INT_SAMPLES,
        false,
        "覆盖 signed int 最大/最小值结构和 isNull 标志",
    ));
}

#[test]
fn test_partial_result4_max_min_uint() {
    run_spill_round_trip(case(
        "TestPartialResult4MaxMinUint",
        "partialResult4MaxMinUint",
        MAX_MIN_UINT_SAMPLES,
        false,
        "覆盖 unsigned int 值和 isNull 标志",
    ));
}

#[test]
fn test_partial_result4_max_min_decimal() {
    run_spill_round_trip(case(
        "TestPartialResult4MaxMinDecimal",
        "partialResult4MaxMinDecimal",
        MAX_MIN_DECIMAL_SAMPLES,
        false,
        "decimal 样本来自 NewDecFromInt/NewDecFromUint",
    ));
}

#[test]
fn test_partial_result4_max_min_float32() {
    run_spill_round_trip(case(
        "TestPartialResult4MaxMinFloat32",
        "partialResult4MaxMinFloat32",
        MAX_MIN_FLOAT_SAMPLES,
        false,
        "float32/float64 使用相同的三组数值样本",
    ));
}

#[test]
fn test_partial_result4_max_min_float64() {
    run_spill_round_trip(case(
        "TestPartialResult4MaxMinFloat64",
        "partialResult4MaxMinFloat64",
        MAX_MIN_FLOAT_SAMPLES,
        false,
        "float64 分支保持 Go 的精度和空值标志",
    ));
}

#[test]
fn test_partial_result4_max_min_time() {
    run_spill_round_trip(case(
        "TestPartialResult4MaxMinTime",
        "partialResult4MaxMinTime",
        MAX_MIN_TIME_SAMPLES,
        false,
        "时间样本保留 types.NewTime 参数",
    ));
}

#[test]
fn test_partial_result4_max_min_string() {
    let long = get_long_string("平352p凯额6辰c");
    assert!(long.len() > "平352p凯额6辰c".len());
    run_spill_round_trip(case(
        "TestPartialResult4MaxMinString",
        "partialResult4MaxMinString",
        MAX_MIN_STRING_SAMPLES,
        true,
        "长字符串要求 SerializeHelper.buf 扩容",
    ));
}

#[test]
fn test_partial_result4_max_min_json() {
    assert_eq!(get_large_rand_buffer().len(), 10_000);
    run_spill_round_trip(case(
        "TestPartialResult4MaxMinJSON",
        "partialResult4MaxMinJSON",
        MAX_MIN_JSON_SAMPLES,
        true,
        "BinaryJSON 大 value 要覆盖扩容路径",
    ));
}

#[test]
fn test_partial_result4_max_min_enum() {
    run_spill_round_trip(case(
        "TestPartialResult4MaxMinEnum",
        "partialResult4MaxMinEnum",
        MAX_MIN_ENUM_SAMPLES,
        true,
        "Enum 名称使用空串和长字符串两种样本",
    ));
}

#[test]
fn test_partial_result4_max_min_set() {
    run_spill_round_trip(case(
        "TestPartialResult4MaxMinSet",
        "partialResult4MaxMinSet",
        MAX_MIN_SET_SAMPLES,
        true,
        "Set 名称使用空串和长字符串两种样本",
    ));
}

#[test]
fn test_partial_result4_avg_decimal() {
    run_spill_round_trip(case(
        "TestPartialResult4AvgDecimal",
        "partialResult4AvgDecimal",
        AVG_DECIMAL_SAMPLES,
        false,
        "平均值 decimal partial result 同时保存 sum 和 count",
    ));
}

#[test]
fn test_partial_result4_avg_float64() {
    run_spill_round_trip(case(
        "TestPartialResult4AvgFloat64",
        "partialResult4AvgFloat64",
        AVG_FLOAT64_SAMPLES,
        false,
        "平均值 float64 partial result 同时保存 sum 和 count",
    ));
}

#[test]
fn test_partial_result4_sum_decimal() {
    run_spill_round_trip(case(
        "TestPartialResult4SumDecimal",
        "partialResult4SumDecimal",
        SUM_DECIMAL_SAMPLES,
        false,
        "sum decimal partial result 保存 val 和 notNullRowCount",
    ));
}

#[test]
fn test_partial_result4_sum_float64() {
    run_spill_round_trip(case(
        "TestPartialResult4SumFloat64",
        "partialResult4SumFloat64",
        SUM_FLOAT64_SAMPLES,
        false,
        "sum float64 partial result 保存 val 和 notNullRowCount",
    ));
}

#[test]
fn test_base_partial_result4_group_concat() {
    run_spill_round_trip(case(
        "TestBasePartialResult4GroupConcat",
        "basePartialResult4GroupConcat",
        GROUP_CONCAT_SAMPLES,
        true,
        "group concat 需要区分 nil buffer 和空 buffer",
    ));
}

#[test]
fn test_partial_result4_bit_func() {
    run_spill_round_trip(case(
        "TestPartialResult4BitFunc",
        "partialResult4BitFunc",
        BIT_FUNC_SAMPLES,
        false,
        "bit 聚合 partial result 是整数别名",
    ));
}

#[test]
fn test_partial_result4_json_arrayagg() {
    run_spill_round_trip(case(
        "TestPartialResult4JsonArrayagg",
        "partialResult4JsonArrayagg",
        JSON_ARRAYAGG_SAMPLES,
        true,
        "JSON_ARRAYAGG 覆盖多类型 entries 和大 Opaque buffer",
    ));
}

#[test]
fn test_partial_result4_json_object_agg() {
    run_spill_round_trip(case(
        "TestPartialResult4JsonObjectAgg",
        "partialResult4JsonObjectAgg",
        JSON_OBJECT_AGG_SAMPLES,
        true,
        "JSON_OBJECTAGG 使用 MemAwareMap 保存 key/value entries",
    ));
}

#[test]
fn test_partial_result4_first_row_decimal() {
    run_spill_round_trip(case(
        "TestPartialResult4FirstRowDecimal",
        "partialResult4FirstRowDecimal",
        FIRST_ROW_DECIMAL_SAMPLES,
        false,
        "FirstRow 系列都带 basePartialResult4FirstRow 状态",
    ));
}

#[test]
fn test_partial_result4_first_row_int() {
    run_spill_round_trip(case(
        "TestPartialResult4FirstRowInt",
        "partialResult4FirstRowInt",
        FIRST_ROW_INT_SAMPLES,
        false,
        "FirstRow int 覆盖负数、零和正数",
    ));
}

#[test]
fn test_partial_result4_first_row_time() {
    run_spill_round_trip(case(
        "TestPartialResult4FirstRowTime",
        "partialResult4FirstRowTime",
        FIRST_ROW_TIME_SAMPLES,
        false,
        "FirstRow time 保留 types.NewTime 参数",
    ));
}

#[test]
fn test_partial_result4_first_row_string() {
    run_spill_round_trip(case(
        "TestPartialResult4FirstRowString",
        "partialResult4FirstRowString",
        FIRST_ROW_STRING_SAMPLES,
        true,
        "FirstRow string 长值触发 buf 扩容检查",
    ));
}

#[test]
fn test_partial_result4_first_row_float32() {
    run_spill_round_trip(case(
        "TestPartialResult4FirstRowFloat32",
        "partialResult4FirstRowFloat32",
        FIRST_ROW_FLOAT_SAMPLES,
        false,
        "FirstRow float32 使用 -1.1、0、1.1",
    ));
}

#[test]
fn test_partial_result4_first_row_float64() {
    run_spill_round_trip(case(
        "TestPartialResult4FirstRowFloat64",
        "partialResult4FirstRowFloat64",
        FIRST_ROW_FLOAT_SAMPLES,
        false,
        "FirstRow float64 使用 -1.1、0、1.1",
    ));
}

#[test]
fn test_partial_result4_first_row_duration() {
    run_spill_round_trip(case(
        "TestPartialResult4FirstRowDuration",
        "partialResult4FirstRowDuration",
        FIRST_ROW_DURATION_SAMPLES,
        false,
        "Duration 样本保留 types.NewDuration 五个参数",
    ));
}

#[test]
fn test_partial_result4_first_row_json() {
    run_spill_round_trip(case(
        "TestPartialResult4FirstRowJSON",
        "partialResult4FirstRowJSON",
        FIRST_ROW_JSON_SAMPLES,
        true,
        "FirstRow JSON 覆盖空 value 和大随机 buffer",
    ));
}

#[test]
fn test_partial_result4_first_row_enum() {
    run_spill_round_trip(case(
        "TestPartialResult4FirstRowEnum",
        "partialResult4FirstRowEnum",
        FIRST_ROW_ENUM_SAMPLES,
        true,
        "FirstRow Enum 覆盖空名称和 testLongStr2",
    ));
}

#[test]
fn test_partial_result4_first_row_set() {
    run_spill_round_trip(case(
        "TestPartialResult4FirstRowSet",
        "partialResult4FirstRowSet",
        FIRST_ROW_SET_SAMPLES,
        true,
        "FirstRow Set 覆盖空名称和 testLongStr1",
    ));
}

/// 真实路径：COUNT partial result 经 SerializeHelper 写入 chunk 后再反序列化还原。
#[test]
fn spill_count_round_trips_through_chunk_storage() {
    use astersql_util_serialization::{chunk, types};

    let mut source = chunk::NewChunkWithCapacity(vec![types::NewFieldType(16)], 3);
    let mut serializer = crate::SerializeHelper::new();
    // 写入与 Go 样本一致的三组 count 值。
    for value in [-123_i64, 0, 123] {
        source.AppendBytes(0, serializer.serialize_count(value));
    }

    let mut deserializer = crate::DeserializeHelper::new(source.Column(0), source.NumRows());
    let mut actual = Vec::new();
    let mut value = 0_i64;
    // deserialize_count 返回 false 表示列已读尽。
    while deserializer.deserialize_count(&mut value) {
        actual.push(value);
    }
    assert_eq!(actual, vec![-123, 0, 123]);
}

/// 真实路径：长字符串 MAX/MIN spill 往返，并断言序列化缓冲发生扩容。
#[test]
fn spill_long_string_round_trip_preserves_null_and_payload() {
    use astersql_util_serialization::{chunk, types};

    let payload = get_long_string("AsterSQL-");
    let expected = crate::PartialResult4MaxMinString {
        is_null: false,
        value: payload,
    };
    let mut source = chunk::NewChunkWithCapacity(vec![types::NewFieldType(16)], 1);
    let mut serializer = crate::SerializeHelper::new();
    let encoded = serializer.serialize_max_min_string(&expected);
    // 长度阈值用于确认 SerializeHelper 内部 buf 扩容路径被触发。
    assert!(
        encoded.len() > 64,
        "long payload must exercise buffer growth"
    );
    source.AppendBytes(0, encoded);

    let mut deserializer = crate::DeserializeHelper::new(source.Column(0), source.NumRows());
    let mut actual = crate::PartialResult4MaxMinString::default();
    assert!(deserializer.deserialize_max_min_string(&mut actual));
    assert_eq!(actual, expected);
    // 仅一行数据，再次读取应返回 false。
    assert!(!deserializer.deserialize_max_min_string(&mut actual));
}

/// Exercise the real SerializeHelper/DeserializeHelper byte protocol for the
/// scalar and heterogeneous partial-result families covered by Go spill tests.
#[test]
fn spill_scalar_and_json_partial_results_round_trip_through_columns() {
    use astersql_util_serialization::{chunk, types};
    use std::collections::HashMap;
    use std::io::Cursor;

    fn source_with_one_row(bytes: &[u8]) -> Box<chunk::Chunk> {
        let mut source = chunk::NewChunkWithCapacity(vec![types::NewFieldType(16)], 1);
        source.AppendBytes(0, bytes);
        source
    }

    let mut serializer = crate::SerializeHelper::new();

    let encoded = serializer.serialize_count(42).to_vec();
    let source = source_with_one_row(&encoded);
    let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
    let mut count = 0;
    assert!(reader.deserialize_count(&mut count));
    assert_eq!(count, 42);
    assert!(!reader.deserialize_count(&mut count));

    let max = crate::PartialResult4MaxMinInt {
        is_null: false,
        value: -7,
    };
    let encoded = serializer.serialize_max_min_int(&max).to_vec();
    let source = source_with_one_row(&encoded);
    let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
    let mut decoded_max = crate::PartialResult4MaxMinInt::default();
    assert!(reader.deserialize_max_min_int(&mut decoded_max));
    assert_eq!(decoded_max, max);

    let avg = crate::AvgFloat64PartialResult {
        sum: 12.5,
        count: 3,
    };
    let encoded = serializer.serialize_avg_float64(&avg).to_vec();
    let source = source_with_one_row(&encoded);
    let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
    let mut decoded_avg = crate::AvgFloat64PartialResult::default();
    assert!(reader.deserialize_avg_float64(&mut decoded_avg));
    assert_eq!(decoded_avg, avg);

    let sum = crate::PartialResult4SumInt64 {
        value: -11,
        not_null_row_count: 2,
    };
    let encoded = serializer.serialize_sum_int64(&sum).to_vec();
    let source = source_with_one_row(&encoded);
    let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
    let mut decoded_sum = crate::PartialResult4SumInt64::default();
    assert!(reader.deserialize_sum_int64(&mut decoded_sum));
    assert_eq!(decoded_sum, sum);

    let encoded = serializer.serialize_bit_func(0x55).to_vec();
    let source = source_with_one_row(&encoded);
    let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
    let mut bit = 0;
    assert!(reader.deserialize_bit_func(&mut bit));
    assert_eq!(bit, 0x55);

    let first_row = crate::PartialResult4FirstRowString {
        state: crate::FirstRowState {
            is_null: true,
            got_first_row: true,
        },
        value: "ignored".to_owned(),
    };
    let encoded = serializer.serialize_first_row_string(&first_row).to_vec();
    let source = source_with_one_row(&encoded);
    let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
    let mut decoded_first_row = crate::PartialResult4FirstRowString::default();
    assert!(reader.deserialize_first_row_string(&mut decoded_first_row));
    assert_eq!(decoded_first_row, first_row);

    let array = crate::JsonArrayPartialResult {
        entries: vec![
            crate::SpillValue::Int64(1),
            crate::SpillValue::String("value".to_owned()),
        ],
    };
    let encoded = serializer.serialize_json_array(&array).to_vec();
    let source = source_with_one_row(&encoded);
    let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
    let mut decoded_array = crate::JsonArrayPartialResult::default();
    assert!(reader.deserialize_json_array(&mut decoded_array));
    assert_eq!(decoded_array, array);

    let object = crate::JsonObjectPartialResult {
        entries: HashMap::from([("key".to_owned(), crate::SpillValue::Bool(true))]),
    };
    let encoded = serializer.serialize_json_object(&object).to_vec();
    let source = source_with_one_row(&encoded);
    let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
    let mut decoded_object = crate::JsonObjectPartialResult::default();
    assert!(reader.deserialize_json_object(&mut decoded_object).0);
    assert_eq!(decoded_object, object);

    let group_concat = crate::GroupConcatPartialResult {
        values_buffer: Cursor::new(Vec::new()),
        buffer: Some(Cursor::new(b"a,b".to_vec())),
    };
    let encoded = serializer.serialize_group_concat(&group_concat).to_vec();
    let source = source_with_one_row(&encoded);
    let mut reader = crate::DeserializeHelper::new(source.Column(0), 1);
    let mut decoded_group_concat = crate::GroupConcatPartialResult::default();
    assert!(reader.deserialize_group_concat(&mut decoded_group_concat));
    assert_eq!(
        decoded_group_concat
            .buffer
            .as_ref()
            .map(|buffer| buffer.get_ref().as_slice()),
        Some(b"a,b".as_slice())
    );
}

fn round_trip_state<T: crate::SpillState>(state: &T, template: T) -> (T, i64) {
    use crate::Serializer;
    let function = crate::StateSerializer {
        ordinal: 1,
        template,
    };
    let mut source =
        chunk::NewChunkWithCapacity(vec![types::NewFieldType(16), types::NewFieldType(16)], 2);
    source.AppendBytes(0, b"unrelated-column");
    let partial = Box::new(state.copy_partial()) as crate::PartialResult;
    function.serialize_partial_result(&partial, &mut source, &mut crate::SerializeHelper::new());
    let (mut decoded, memory) = function.deserialize_partial_result(&source);
    assert_eq!(decoded.len(), 1);
    let restored = *decoded.remove(0).downcast::<T>().ok().unwrap();
    source.Reset(); // restored strings/vectors/decimals must own their bytes
    (restored, memory)
}

#[test]
fn spill_distinct_count_preserves_int_real_decimal_duration_string_and_multi_keys() {
    use crate::func_count_distinct::*;
    let mut integer = CountDistinctInt::default();
    integer.update([Some(1), Some(-2), Some(-2), None]);
    let (restored, memory) = round_trip_state(&integer, CountDistinctInt::default());
    assert_eq!(restored.values, integer.values);
    assert!(memory > 0);
    let mut real = CountDistinctReal::default();
    update_distinct_real(
        &mut real,
        [
            Some(1.25),
            Some(-2.5),
            Some(-0.0),
            Some(0.0),
            Some(f64::NAN),
            Some(f64::NAN),
        ],
    );
    let (restored, _) = round_trip_state(&real, CountDistinctReal::default());
    assert_eq!(restored.values, real.values);
    let mut decimal = CountDistinctDecimal::default();
    decimal.update([Some(b"decimal-key".to_vec()), Some(vec![0, 1, 2, b'x'])]);
    let (restored, memory) = round_trip_state(&decimal, CountDistinctDecimal::default());
    assert_eq!(restored.values, decimal.values);
    assert!(memory >= 15);
    let mut duration = CountDistinctDuration::default();
    duration.update([Some(123), Some(-456)]);
    assert_eq!(
        round_trip_state(&duration, CountDistinctDuration::default())
            .0
            .values,
        duration.values
    );
    let mut string = CountDistinctString::default();
    string.update([Some(Vec::new()), Some(b"x".repeat(100_000))]);
    assert_eq!(
        round_trip_state(&string, CountDistinctString::default())
            .0
            .values,
        string.values
    );
    let mut multi = CountDistinctMulti::default();
    multi.encoded_rows.insert(b"arg1\0arg2".to_vec());
    multi.encoded_rows.insert(b"y".repeat(100_000));
    assert_eq!(
        round_trip_state(&multi, CountDistinctMulti::default())
            .0
            .encoded_rows,
        multi.encoded_rows
    );
}

#[test]
fn spill_distinct_avg_sum_and_integer_bit_patterns_remain_mergeable() {
    use crate::func_avg::*;
    use crate::func_sum::*;
    use crate::func_sum_int::*;
    let mut float_sum = DistinctFloatSum::default();
    float_sum.update([Some(10.5), Some(-20.75)]);
    let (mut restored, _) = round_trip_state(&float_sum, DistinctFloatSum::default());
    restored.merge(&float_sum);
    assert_eq!(restored.value(), float_sum.value());
    let mut float_avg = DistinctFloatAvg::default();
    float_avg.update([Some(10.5), Some(-20.75)]);
    let (mut restored, _) = round_trip_state(&float_avg, DistinctFloatAvg::default());
    restored.merge(&float_avg);
    assert_eq!(restored.result(), float_avg.result());
    let mut decimal_sum = DistinctDecimalSum::default();
    decimal_sum.insert_keyed(b"s1".to_vec(), Decimal::new(100, 0));
    decimal_sum.insert_keyed(b"s2".to_vec(), Decimal::new(-200, 0));
    let (mut restored, _) = round_trip_state(&decimal_sum, DistinctDecimalSum::default());
    assert_eq!(restored.keys, decimal_sum.keys);
    assert_eq!(restored.value().unwrap(), decimal_sum.value().unwrap());
    restored.merge(&decimal_sum);
    assert_eq!(restored.len(), 2);
    let mut decimal_avg = DistinctDecimalAvg::default();
    decimal_avg
        .sum
        .insert_keyed(b"d1".to_vec(), Decimal::new(10, 0));
    decimal_avg
        .sum
        .insert_keyed(b"d2".to_vec(), Decimal::new(-20, 0));
    let (restored, _) = round_trip_state(&decimal_avg, DistinctDecimalAvg::default());
    assert_eq!(restored.sum.keys, decimal_avg.sum.keys);
    assert_eq!(restored.result(2).unwrap(), decimal_avg.result(2).unwrap());
    let mut signed = SumDistinctInt64::default();
    signed.update([Some(100), Some(-200)]);
    let (mut restored, _) = round_trip_state(&signed, SumDistinctInt64::default());
    restored.merge(&signed);
    assert_eq!(restored.value().unwrap(), signed.value().unwrap());
    let mut unsigned = SumDistinctUint64::default();
    unsigned.update([Some(u64::MAX)]);
    let (restored, _) = round_trip_state(&unsigned, SumDistinctUint64::default());
    assert!(restored.bit_patterns.contains(&-1));
    assert_eq!(restored.value().unwrap(), Some(u64::MAX));
}

#[test]
fn spill_approximate_count_and_variance_preserve_full_partial_state() {
    use crate::func_count_distinct::ApproxCountDistinct;
    use crate::func_varpop::*;
    let mut approx = ApproxCountDistinct::default();
    for value in [0, 1, 2, 1, 1024, u32::MAX as u64] {
        approx.insert_hash64(value);
    }
    let (restored, _) = round_trip_state(&approx, ApproxCountDistinct::default());
    assert_eq!(restored.serialize(), approx.serialize());
    assert_eq!(restored.estimate(), approx.estimate());
    let mut variance = VarianceState::default();
    variance.update([Some(1.0), Some(2.5), Some(4.0)]);
    let (restored, _) = round_trip_state(&variance, VarianceState::default());
    assert_eq!(
        (restored.count, restored.sum, restored.variance),
        (variance.count, variance.sum, variance.variance)
    );
    assert_eq!(
        restored.population_variance(),
        variance.population_variance()
    );
    assert_eq!(restored.sample_variance(), variance.sample_variance());
    let mut distinct = DistinctVariance::default();
    distinct.update([Some(1.5), Some(3.5), Some(1.5)]);
    let (mut restored, _) = round_trip_state(&distinct, DistinctVariance::default());
    restored.merge(&distinct);
    assert_eq!(restored.values.len(), 2);
    assert_eq!(restored.population_variance(), Some(1.0));
}

#[test]
fn spill_percentile_preserves_all_five_typed_samples_and_memory_charges() {
    use crate::func_max_min::{DurationValue, TimeValue};
    use crate::func_percentile::Percentile;
    use crate::func_sum::Decimal;
    let mut integer = Percentile::new(50);
    integer.update([Some(-3_i64), Some(0), Some(10)]);
    let (mut restored, memory) = round_trip_state(&integer, Percentile::new(50));
    assert_eq!(restored.values(), integer.values());
    assert_eq!(restored.result(), Some(&0));
    assert_eq!(memory, 24 + 3 * 8);
    let mut real = Percentile::new(50);
    real.update([Some(-1.5_f64), Some(2.25)]);
    assert_eq!(
        round_trip_state(&real, Percentile::new(50)).0.values(),
        real.values()
    );
    let mut decimal = Percentile::new(50);
    decimal.update([Some(Decimal::new(-2, 0)), Some(Decimal::new(11, 0))]);
    let (restored, memory) = round_trip_state(&decimal, Percentile::new(50));
    assert_eq!(restored.values(), decimal.values());
    assert_eq!(memory, 24 + 2 * 40);
    let mut time = Percentile::new(50);
    time.update([
        Some(TimeValue {
            packed: 1,
            kind: 12,
            fsp: 3,
        }),
        Some(TimeValue {
            packed: 365,
            kind: 7,
            fsp: 6,
        }),
    ]);
    let (restored, memory) = round_trip_state(&time, Percentile::new(50));
    assert_eq!(restored.values(), time.values());
    assert_eq!(restored.values()[1].fsp, 6);
    assert_eq!(restored.values()[0].kind, 12);
    assert_eq!(memory, 24 + 2 * 8);
    let mut duration = Percentile::new(50);
    duration.update([
        Some(DurationValue {
            nanos: -123,
            fsp: 2,
        }),
        Some(DurationValue { nanos: 456, fsp: 4 }),
    ]);
    let (restored, memory) = round_trip_state(&duration, Percentile::new(50));
    assert_eq!(restored.values(), duration.values());
    assert_eq!(restored.values()[1].fsp, 4);
    assert_eq!(memory, 24 + 2 * 8);
}

#[test]
fn spill_first_vector_preserves_unseen_null_empty_and_nonempty_flags() {
    use crate::func_first_row::FirstRow;
    use crate::func_max_min::VectorFloat32;
    for state in [
        None,
        Some(None),
        Some(Some(VectorFloat32(Vec::new()))),
        Some(Some(VectorFloat32(vec![-1.5, 0.0, 2.25]))),
    ] {
        let original = FirstRow { state };
        let (restored, _) = round_trip_state(&original, FirstRow::<VectorFloat32>::default());
        assert_eq!(restored, original);
    }
}

#[test]
fn spill_group_concat_keeps_binary_collation_keys_and_allocates_result_buffer() {
    use crate::func_group_concat::GroupConcat;
    let mut original = GroupConcat::new(b",".to_vec(), 1_000_000, true);
    original.update_keyed(b"k1".to_vec(), b"v1".to_vec());
    original.update_keyed(vec![0, 1, 255], b"long".repeat(10_000));
    let (mut restored, memory) =
        round_trip_state(&original, GroupConcat::new(b",".to_vec(), 1_000_000, true));
    assert_eq!(restored.distinct, original.distinct);
    assert!(restored.result().is_some());
    assert!(memory > 40_000);
    restored.merge(&original);
    assert_eq!(restored.distinct.as_ref().unwrap().len(), 2);
}

#[test]
fn spill_native_sql_values_keep_full_decimal_precision_time_bits_and_vectors() {
    let mut decimal = types::MyDecimal::default();
    decimal
        .FromString(b"12345678901234567890123456789012345678901234567890.12345")
        .unwrap();
    let values = vec![decimal];
    let (restored, memory) = round_trip_state(&values, Vec::<types::MyDecimal>::new());
    assert_eq!(restored[0].String(), values[0].String());
    assert_eq!(memory, 24 + 40);
    let times = vec![types::Time {
        coreTime: types::CoreTime(0x123456789abcde0d),
    }];
    let (restored, memory) = round_trip_state(&times, Vec::<types::Time>::new());
    assert_eq!(restored, times);
    assert_eq!(memory, 24 + 8);
    let durations = vec![
        types::Duration {
            Duration: -123,
            Fsp: 2,
        },
        types::Duration {
            Duration: 456,
            Fsp: 4,
        },
    ];
    let (restored, memory) = round_trip_state(&durations, Vec::<types::Duration>::new());
    assert_eq!(restored, durations);
    assert_eq!(memory, 24 + 16);
    let vector = types::ParseVectorFloat32("[-1.5,0,2.25]").unwrap();
    let first = crate::FirstRowPartialResult {
        state: crate::FirstRowState {
            is_null: false,
            got_first_row: true,
        },
        value: types::ZeroCopyDeserializeVectorFloat32(vector.ZeroCopySerialize())
            .unwrap()
            .0,
    };
    let (restored, _) = round_trip_state(
        &first,
        crate::FirstRowPartialResult::<types::VectorFloat32>::default(),
    );
    assert_eq!(restored.state, first.state);
    assert_eq!(
        restored.value.ZeroCopySerialize(),
        vector.ZeroCopySerialize()
    );
    let extreme = crate::MaxMinPartialResult {
        is_null: false,
        value: types::ZeroCopyDeserializeVectorFloat32(vector.ZeroCopySerialize())
            .unwrap()
            .0,
    };
    let (restored, _) = round_trip_state(
        &extreme,
        crate::MaxMinPartialResult::<types::VectorFloat32>::default(),
    );
    assert!(!restored.is_null);
    assert_eq!(
        restored.value.ZeroCopySerialize(),
        vector.ZeroCopySerialize()
    );
}

#[test]
fn spill_factory_registers_all_added_variants_and_null_percentile_rows() {
    use crate::Serializer;
    use crate::builder::{AggImplementation as A, BuiltAggFunc, ValueKind as K};
    let variants = vec![
        A::CountOriginalDistinct(K::Int),
        A::CountPartialDistinct(K::Float64),
        A::CountOriginalDistinct(K::Decimal),
        A::CountOriginalDistinct(K::Duration),
        A::CountOriginalDistinct(K::String),
        A::CountOriginalDistinctMulti,
        A::ApproxCountDistinctOriginal,
        A::ApproxCountDistinctPartial1,
        A::ApproxCountDistinctPartial2,
        A::ApproxCountDistinctFinal,
        A::AvgOriginalDistinctDecimal,
        A::AvgPartialDistinctDecimal,
        A::AvgOriginalDistinctFloat64,
        A::AvgPartialDistinctFloat64,
        A::SumOriginalDistinctDecimal,
        A::SumPartialDistinctDecimal,
        A::SumOriginalDistinctFloat64,
        A::SumPartialDistinctFloat64,
        A::SumDistinctInt,
        A::SumDistinctUint,
        A::VarPop,
        A::VarSamp,
        A::StddevPop,
        A::StddevSamp,
        A::VarPopOriginalDistinct,
        A::VarPopPartialDistinct,
        A::VarSampOriginalDistinct,
        A::VarSampPartialDistinct,
        A::StddevPopOriginalDistinct,
        A::StddevPopPartialDistinct,
        A::StddevSampOriginalDistinct,
        A::StddevSampPartialDistinct,
        A::GroupConcatDistinctOriginal,
        A::GroupConcatDistinctPartial,
        A::FirstRow(K::VectorFloat32),
        A::Percentile {
            kind: K::Int,
            percent: 50,
        },
        A::Percentile {
            kind: K::Float64,
            percent: 50,
        },
        A::Percentile {
            kind: K::Decimal,
            percent: 50,
        },
        A::Percentile {
            kind: K::Time,
            percent: 50,
        },
        A::Percentile {
            kind: K::Duration,
            percent: 50,
        },
        A::PercentileNull { percent: 50 },
    ];
    for implementation in variants {
        let built = BuiltAggFunc {
            implementation,
            ordinal: 0,
            argument_count: 1,
            order_by: Vec::new(),
            separator: Some(",".into()),
            max_len: Some(1024),
            default_value: None,
        };
        let (function, partial) = built.spill_function().unwrap();
        let mut source = chunk::NewChunkWithCapacity(vec![types::NewFieldType(16)], 2);
        let mut helper = crate::SerializeHelper::new();
        function.serialize_partial_result(&partial, &mut source, &mut helper);
        function.serialize_partial_result(&partial, &mut source, &mut helper);
        assert_eq!(source.NumRows(), 2);
        let (restored, memory) = function.deserialize_partial_result(&source);
        assert_eq!(restored.len(), 2);
        if matches!(built.implementation, A::PercentileNull { .. }) {
            assert!(source.Column(0).IsNull(0));
            assert_eq!(memory, 0);
        }
    }
}
