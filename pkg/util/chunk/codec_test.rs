// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// `codec` 编解码与类型宽度估算测试。
//
// 对齐 Go `codec_test.go` 的固定长、变长、NULL、DecodeToChunk 与宽度估算场景，
// 并验证原生 i64 字节转换在边界值上可无损往返。

use super::*;

fn field_type(tp: u8) -> types::FieldType {
    *types::NewFieldType(tp)
}

/// 覆盖有符号 i64 切片与字节缓冲之间的往返转换。
#[test]
fn native_i64_codec_round_trips_signed_values() {
    // 边界与典型值一并编码再解码，确认字节布局正确。
    let values = [i64::MIN, -1, 0, 42, i64::MAX];
    assert_eq!(bytesToI64Slice(&i64SliceToBytes(&values)), values);
}

/// 对齐 Go TestCodec：混合固定长、变长与 NULL 列应完整往返，且不遗留输入。
#[test]
fn codec_decode_to_chunk_matches_go_mixed_columns() {
    let col_types = vec![
        field_type(mysql::TypeLonglong),
        field_type(mysql::TypeLonglong),
        field_type(mysql::TypeVarchar),
        field_type(mysql::TypeVarchar),
    ];
    let mut nulls = newFixedLenColumn(8, 10);
    let mut integers = newFixedLenColumn(8, 10);
    let mut strings = newVarLenColumn(10);
    let mut bytes = newVarLenColumn(10);
    for i in 0..10_i64 {
        nulls.AppendNull();
        integers.AppendInt64(i);
        strings.AppendString(&format!("{i}.12345"));
        bytes.AppendBytes(&[i as u8, 0, 0xff]);
    }
    let old_chunk = Chunk {
        columns: vec![*nulls, *integers, *strings, *bytes],
        requiredRows: 10,
        ..Chunk::default()
    };

    let codec = NewCodec(col_types);
    let encoded = codec.Encode(&old_chunk);
    let mut new_chunk = Chunk {
        columns: vec![
            *newFixedLenColumn(8, 10),
            *newFixedLenColumn(8, 10),
            *newVarLenColumn(10),
            *newVarLenColumn(10),
        ],
        requiredRows: 10,
        ..Chunk::default()
    };
    let remained = codec.DecodeToChunk(&encoded, &mut new_chunk);

    assert!(remained.is_empty());
    assert_eq!(new_chunk.NumCols(), 4);
    assert_eq!(new_chunk.NumRows(), 10);
    for i in 0..10 {
        assert!(new_chunk.columns[0].IsNull(i));
        assert!(!new_chunk.columns[1].IsNull(i));
        assert_eq!(new_chunk.columns[1].GetInt64(i), i as i64);
        assert_eq!(new_chunk.columns[2].GetString(i), format!("{i}.12345"));
        assert_eq!(new_chunk.columns[3].GetBytes(i), &[i as u8, 0, 0xff]);
    }
}

/// 对齐 Go TestEstimateTypeWidth 的所有固定、分段与未知长度分支。
#[test]
fn estimate_type_width_matches_go_boundaries() {
    assert_eq!(EstimateTypeWidth(&field_type(mysql::TypeLonglong)), 8);

    let mut string_type = field_type(mysql::TypeString);
    string_type.SetFlen(31);
    assert_eq!(EstimateTypeWidth(&string_type), 31);
    string_type.SetFlen(999);
    assert_eq!(EstimateTypeWidth(&string_type), 515);
    string_type.SetFlen(2000);
    assert_eq!(EstimateTypeWidth(&string_type), 516);

    let unknown_string_type = field_type(mysql::TypeString);
    assert_eq!(EstimateTypeWidth(&unknown_string_type), 32);
}
