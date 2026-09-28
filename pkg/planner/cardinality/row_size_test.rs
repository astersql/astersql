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

// 平均列宽估算的单元测试。
//
// 覆盖 `AvgColSize`、`AvgColSizeChunkFormat`、`AvgColSizeDataInDiskByRows`
// 在定长/变长类型、全 NULL 列以及多行平均宽度下的 Go 对齐行为。

use crate::*;

/// 构造带单个桶的已分析列直方图，便于控制 TotColSize / NullCount。
fn analyzed_column(
    field_type: types::FieldType,
    row_count: i64,
    null_count: i64,
    total_column_size: i64,
) -> statistics::Column {
    let mut histogram = statistics::NewHistogram(
        1,
        row_count - null_count,
        null_count,
        1,
        &field_type,
        1,
        total_column_size,
    );
    // 非空行时塞入一个覆盖全部非空值的桶，模拟完整分析结果。
    if row_count > null_count {
        histogram.Buckets.push(statistics::Bucket {
            Count: row_count - null_count,
            Repeat: 1,
            NDV: row_count - null_count,
        });
    }
    statistics::Column {
        CMSketch: None,
        TopN: None,
        FMSketch: None,
        Info: None,
        Histogram: histogram,
        StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
        PhysicalID: 1,
        StatsVer: statistics::Version2 as i64,
        IsHandle: false,
    }
}

/// 核对单行与多行场景下三种列宽公式与 Go 期望值一致。
#[test]
fn test_avg_col_len() {
    crate::main_test::setup_for_cardinality_test();
    let int_type = *types::NewFieldType(mysql::TypeLonglong);
    let varchar_type = *types::NewFieldType(mysql::TypeVarchar);
    let float_type = *types::NewFieldType(mysql::TypeFloat);
    let datetime_type = *types::NewFieldType(mysql::TypeDatetime);

    // --- 单行统计：定长类型走固定宽度，变长类型走 TotColSize ---
    let integer = analyzed_column(int_type.clone(), 1, 0, 1);
    let varchar = analyzed_column(varchar_type.clone(), 1, 0, 8);
    let float = analyzed_column(float_type.clone(), 1, 0, 8);
    let datetime = analyzed_column(datetime_type.clone(), 1, 0, 8);
    let null_varchar = analyzed_column(varchar_type.clone(), 1, 1, 0);

    assert_eq!(1.0, AvgColSize(&integer, 1, false));
    assert_eq!(8.0, AvgColSizeDataInDiskByRows(&integer, 1));
    assert_eq!(8.0, AvgColSizeChunkFormat(&integer, 1));

    assert_eq!(8.0, AvgColSize(&varchar, 1, false));
    assert_eq!(8.0, AvgColSize(&float, 1, false));
    assert_eq!(8.0, AvgColSize(&datetime, 1, false));
    assert_eq!(5.0, AvgColSizeDataInDiskByRows(&varchar, 1));
    assert_eq!(
        std::mem::size_of::<f32>() as f64,
        AvgColSizeDataInDiskByRows(&float, 1)
    );
    assert_eq!(
        chunk::GetFixedLen(&datetime_type) as f64,
        AvgColSizeDataInDiskByRows(&datetime, 1)
    );
    assert_eq!(13.0, AvgColSizeChunkFormat(&varchar, 1));
    assert_eq!(
        std::mem::size_of::<f32>() as f64,
        AvgColSizeChunkFormat(&float, 1)
    );
    assert_eq!(
        chunk::GetFixedLen(&datetime_type) as f64,
        AvgColSizeChunkFormat(&datetime, 1)
    );
    // 全 NULL 变长列：chunk 仍保留 offsets 开销，磁盘格式则为 0。
    assert_eq!(8.0, AvgColSizeChunkFormat(&null_varchar, 1));
    assert_eq!(0.0, AvgColSizeDataInDiskByRows(&null_varchar, 1));

    // --- 两行统计：验证平均值与 Log2 修正的变长宽度 ---
    let integer = analyzed_column(int_type, 2, 0, 3);
    let varchar = analyzed_column(varchar_type.clone(), 2, 0, 21);
    let float = analyzed_column(float_type, 2, 0, 16);
    let datetime = analyzed_column(datetime_type.clone(), 2, 0, 16);
    let null_varchar = analyzed_column(varchar_type, 2, 2, 0);
    let variable_width = ((10.5 - 10.5_f64.log2()) * 100.0).round() / 100.0;

    assert_eq!(1.5, AvgColSize(&integer, 2, false));
    assert_eq!(10.5, AvgColSize(&varchar, 2, false));
    assert_eq!(8.0, AvgColSize(&float, 2, false));
    assert_eq!(8.0, AvgColSize(&datetime, 2, false));
    assert_eq!(8.0, AvgColSizeDataInDiskByRows(&integer, 2));
    assert_eq!(variable_width, AvgColSizeDataInDiskByRows(&varchar, 2));
    assert_eq!(
        std::mem::size_of::<f32>() as f64,
        AvgColSizeDataInDiskByRows(&float, 2)
    );
    assert_eq!(
        chunk::GetFixedLen(&datetime_type) as f64,
        AvgColSizeDataInDiskByRows(&datetime, 2)
    );
    assert_eq!(8.0, AvgColSizeChunkFormat(&integer, 2));
    assert_eq!(variable_width + 8.0, AvgColSizeChunkFormat(&varchar, 2));
    assert_eq!(
        std::mem::size_of::<f32>() as f64,
        AvgColSizeChunkFormat(&float, 2)
    );
    assert_eq!(
        chunk::GetFixedLen(&datetime_type) as f64,
        AvgColSizeChunkFormat(&datetime, 2)
    );
    assert_eq!(8.0, AvgColSizeChunkFormat(&null_varchar, 2));
    assert_eq!(0.0, AvgColSizeDataInDiskByRows(&null_varchar, 2));
}
