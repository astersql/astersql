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

// Codec / Column 编解码、比较与类型宽度估算的补充单元测试（对齐 Go）。
//
// 覆盖：nullBitmap 重建、Codec 往返、Decoder 跨 bitmap 边界增量解码、
// 浮点 NaN/NULL 比较与二分查找、Decimal/Time 存储及 EstimateTypeWidth。

use super::*;

/// 按 MySQL 类型码构造 `FieldType`，便于测试固定/变长列路径。
fn field_type(tp: u8) -> types::FieldType {
    *types::NewFieldType(tp)
}

/// 验证 AppendCellNTimes、SetNull 与 CopyReconstruct 对 bitmap/取值的影响。
#[test]
fn column_bitmap_and_reconstruction_match_go() {
    let mut source = newFixedLenColumn(8, 1);
    source.AppendInt64(42);

    let mut repeated = newFixedLenColumn(8, 8);
    // 将同一单元格重复 8 次，nullBitmap 应全为 1（0xff）。
    repeated.AppendCellNTimes(&source, 0, 8);
    assert_eq!(repeated.Rows(), 8);
    assert_eq!(repeated.nullBitmap, vec![0xff]);
    assert_eq!(repeated.Int64s(), vec![42; 8]);

    repeated.SetNull(3, true);
    // 按物理行下标重排：选中的首行应为 NULL。
    let selected = repeated.CopyReconstruct(Some(&[3, 1, 7]), None);
    assert!(selected.IsNull(0));
    assert!(!selected.IsNull(1));
    assert_eq!(selected.Int64s(), vec![42, 42, 42]);
}

/// 验证固定长、变长与 NULL 列经 Encode/Decode 后语义与 Go 一致。
#[test]
fn codec_round_trip_preserves_fixed_variable_and_null_columns() {
    let types = vec![
        field_type(mysql::TypeLonglong),
        field_type(mysql::TypeVarchar),
    ];
    let mut integers = newFixedLenColumn(8, 3);
    integers.AppendInt64(-7);
    integers.AppendNull();
    integers.AppendInt64(99);

    let mut strings = newVarLenColumn(3);
    strings.AppendString("alpha");
    strings.AppendNull();
    strings.AppendBytes(&[0, 1, 2, 255]);

    let chunk = Chunk {
        columns: vec![*integers, *strings],
        requiredRows: 3,
        ..Chunk::default()
    };
    let codec = NewCodec(types.clone());
    let encoded = codec.Encode(&chunk);
    let (decoded, remained) = codec.Decode(&encoded);

    assert!(remained.is_empty());
    assert_eq!(decoded.NumRows(), 3);
    // Go's fixed-length NULL slot retains the current elemBuf bytes; nullBitmap
    // is the sole authority for whether that payload is visible.
    // 固定长 NULL 槽位仍保留 elemBuf 字节；是否可见只看 nullBitmap。
    assert_eq!(decoded.columns[0].Int64s(), vec![-7, -7, 99]);
    assert!(decoded.columns[0].IsNull(1));
    assert_eq!(decoded.columns[1].GetString(0), "alpha");
    assert!(decoded.columns[1].IsNull(1));
    assert_eq!(decoded.columns[1].GetBytes(2), &[0, 1, 2, 255]);

    let reencoded = codec.Encode(&decoded);
    assert_eq!(reencoded, encoded);
}

/// 验证 Decoder 可跨多次 Decode 追加行，并正确处理 nullBitmap 边界。
#[test]
fn decoder_appends_rows_across_bitmap_boundaries() {
    let types = vec![field_type(mysql::TypeLonglong)];
    let mut source = newFixedLenColumn(8, 10);
    for value in 0..10 {
        if value == 4 {
            source.AppendNull();
        } else {
            source.AppendInt64(value);
        }
    }
    let source_chunk = Chunk {
        columns: vec![*source],
        requiredRows: 10,
        ..Chunk::default()
    };
    let encoded = NewCodec(types.clone()).Encode(&source_chunk);

    let intermediate = Box::new(Chunk {
        columns: vec![*newFixedLenColumn(8, 10)],
        ..Chunk::default()
    });
    let mut decoder = NewDecoder(intermediate, types);
    decoder.Reset(&encoded);

    let mut destination = Chunk {
        columns: vec![*newFixedLenColumn(8, 10)],
        requiredRows: 3,
        ..Chunk::default()
    };
    // 首次 Decode 会把 requiredRows 上取整到 8 的倍数以优化 bitmap 拷贝。
    decoder.Decode(&mut destination);
    assert_eq!(destination.NumRows(), 8);
    assert_eq!(decoder.RemainedRows(), 2);
    destination.requiredRows = 10;
    decoder.Decode(&mut destination);
    assert_eq!(destination.NumRows(), 10);
    assert!(destination.columns[0].IsNull(4));
    assert_eq!(destination.columns[0].GetInt64(9), 9);
    assert!(decoder.IsFinished());
}

/// 验证比较函数对 NaN/NULL 的顺序，以及 LowerBound/UpperBound 二分语义。
#[test]
fn comparison_handles_null_nan_and_binary_search_like_go() {
    let mut floats = newFixedLenColumn(8, 3);
    floats.AppendFloat64(f64::NAN);
    floats.AppendFloat64(-1.0);
    floats.AppendFloat64(2.0);
    let float_chunk = Chunk {
        columns: vec![*floats],
        ..Chunk::default()
    };
    let float_cmp = GetCompareFunc(&field_type(mysql::TypeDouble)).unwrap();
    // Go 中 NaN 比较按约定小于普通浮点；同 NaN 比较相等。
    assert_eq!(
        float_cmp(float_chunk.GetRow(0), 0, float_chunk.GetRow(1), 0),
        -1
    );
    assert_eq!(
        float_cmp(float_chunk.GetRow(0), 0, float_chunk.GetRow(0), 0),
        0
    );

    let mut integers = newFixedLenColumn(8, 5);
    for value in [1, 2, 2, 2, 5] {
        integers.AppendInt64(value);
    }
    let integer_chunk = Chunk {
        columns: vec![*integers],
        ..Chunk::default()
    };
    let needle = types::NewIntDatum(2);
    assert_eq!(integer_chunk.LowerBound(0, &needle), (1, true));
    assert_eq!(integer_chunk.UpperBound(0, &needle), 4);

    let mut nullable = newFixedLenColumn(8, 1);
    nullable.AppendNull();
    let nullable_chunk = Chunk {
        columns: vec![*nullable],
        ..Chunk::default()
    };
    // NULL 小于任意非 NULL 值。
    let int_cmp = GetCompareFunc(&field_type(mysql::TypeLonglong)).unwrap();
    assert_eq!(
        int_cmp(nullable_chunk.GetRow(0), 0, integer_chunk.GetRow(0), 0),
        -1
    );
}

/// 验证 Decimal/Time 按固定宽写入，以及 VARCHAR 平均宽度估算规则。
#[test]
fn native_decimal_time_and_width_rules_match_go() {
    let mut decimal = types::MyDecimal::default();
    decimal.FromString(b"-12345.6789").unwrap();
    let mut decimals = newFixedLenColumn(types::MyDecimalStructSize, 1);
    decimals.AppendMyDecimal(&decimal);
    assert_eq!(decimals.GetDecimal(0), decimal);

    let time = types::NewTime(
        types::CoreTime(0x1234_5678_9abc_def0),
        mysql::TypeDatetime,
        3,
    );
    let mut times = newFixedLenColumn(sizeTime, 1);
    times.AppendTime(time);
    assert_eq!(times.GetTime(0), time);

    // 未声明 flen 时默认 32；中等 flen 用折中公式；固定长类型直接返回宽度。
    let mut varchar = field_type(mysql::TypeVarchar);
    assert_eq!(EstimateTypeWidth(&varchar), 32);
    varchar.SetFlen(64);
    assert_eq!(EstimateTypeWidth(&varchar), 48);
    assert_eq!(EstimateTypeWidth(&field_type(mysql::TypeLonglong)), 8);
}
