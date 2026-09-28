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

// 数据库列类型到 Parquet 类型映射的回归测试。
//
// 覆盖常用 MySQL 类型、未专门支持的类型兜底，以及 DECIMAL 精度边界对应的
// 物理存储宽度，确保 Rust 实现与 Go 版本的导出约定保持一致。

use crate::ColumnInfo;
use crate::column_type::{
    LogicalType, PhysicalType, TimeUnit, decimal_fixed_length_bytes_for_precision, to_column_type,
};

// 构造只保留类型映射所需字段的列元信息，避免各用例重复无关配置。
fn info(name: &str, precision: i32, scale: i32) -> ColumnInfo {
    ColumnInfo {
        name: "c".into(),
        database_type_name: name.into(),
        nullable: false,
        precision,
        scale,
    }
}

// 验证常用数据库类型名、别名和兜底类型的物理/逻辑类型约定。
#[test]
fn to_column_type_mappings_match_database_sql_names() {
    assert_eq!(
        to_column_type(&info("FLOAT", 0, 0)).physical,
        PhysicalType::Double
    );
    assert_eq!(
        to_column_type(&info("CHAR", 0, 0)).logical,
        LogicalType::String
    );
    assert_eq!(
        to_column_type(&info("BLOB", 0, 0)).logical,
        LogicalType::None
    );
    let numeric = to_column_type(&info("NUMERIC", 10, 2));
    assert_eq!(numeric.physical, PhysicalType::ByteArray);
    assert_eq!(numeric.logical, LogicalType::None);
    assert_eq!(
        to_column_type(&info("UNSIGNED MEDIUMINT", 0, 0)).physical,
        PhysicalType::ByteArray
    );
    assert_eq!(
        to_column_type(&info("BIGINT", 0, 0)).physical,
        PhysicalType::Int64
    );
    assert_eq!(
        to_column_type(&info("INTEGER", 0, 0)).physical,
        PhysicalType::ByteArray
    );

    // Driver 不会报告的 MariaDB 别名及未知类型必须走无逻辑注解的兜底分支。
    for name in ["REAL", "BOOL", "NCHAR", "SOME_NEW_TYPE", "INT8", "FIXED"] {
        let fallback = to_column_type(&info(name, 0, 0));
        assert_eq!(fallback.physical, PhysicalType::ByteArray, "{name}");
        assert_eq!(fallback.logical, LogicalType::None, "{name}");
    }

    let enum_type = to_column_type(&info("ENUM", 0, 0));
    assert_eq!(enum_type.physical, PhysicalType::ByteArray);
    assert_eq!(enum_type.logical, LogicalType::String);
    assert_eq!(
        to_column_type(&info("DOUBLE", 0, 0)).physical,
        PhysicalType::Double
    );

    // 时间类型无论声明的 scale 为何，都按未调整 UTC 的微秒时间戳导出。
    for scale in [0, 3] {
        let timestamp = to_column_type(&info("TIMESTAMP", 0, scale));
        assert_eq!(timestamp.physical, PhysicalType::Int64);
        assert_eq!(
            timestamp.logical,
            LogicalType::Timestamp {
                adjusted_to_utc: false,
                unit: TimeUnit::Micros,
            }
        );
    }
    for (precision, scale) in [(0, 6), (23, 3)] {
        let datetime = to_column_type(&info("DATETIME", precision, scale));
        assert_eq!(datetime.physical, PhysicalType::Int64);
        assert_eq!(
            datetime.logical,
            LogicalType::Timestamp {
                adjusted_to_utc: false,
                unit: TimeUnit::Micros,
            }
        );
    }

    // UNSIGNED BIGINT 的完整取值范围需要用 DECIMAL(20,0) 的 9 字节定长数组承载。
    let unsigned_bigint = to_column_type(&info("UNSIGNED BIGINT", 0, 0));
    assert_eq!(unsigned_bigint.physical, PhysicalType::FixedLenByteArray);
    assert_eq!(unsigned_bigint.type_length, 9);
    assert_eq!(
        unsigned_bigint.logical,
        LogicalType::Decimal {
            precision: 20,
            scale: 0,
        }
    );
}

// 验证 DECIMAL 在 9、18 和 38 位精度边界上的存储宽度及非法精度兜底行为。
#[test]
fn decimal_precision_boundaries_match_parquet_storage_widths() {
    let int32 = to_column_type(&info("DECIMAL", 9, 3));
    assert_eq!(int32.physical, PhysicalType::Int32);
    assert_eq!(int32.precision, 9);
    assert_eq!(int32.scale, 3);
    assert_eq!(
        int32.logical,
        LogicalType::Decimal {
            precision: 9,
            scale: 3,
        }
    );

    let int64 = to_column_type(&info("DECIMAL", 18, 4));
    assert_eq!(int64.physical, PhysicalType::Int64);
    assert_eq!(int64.precision, 18);
    assert_eq!(int64.scale, 4);
    let fixed = to_column_type(&info("DECIMAL", 19, 5));
    assert_eq!(fixed.physical, PhysicalType::FixedLenByteArray);
    assert_eq!(
        fixed.type_length,
        decimal_fixed_length_bytes_for_precision(19)
    );
    assert_eq!(fixed.precision, 19);
    assert_eq!(fixed.scale, 5);

    let max_decimal = to_column_type(&info("DECIMAL", 38, 6));
    assert_eq!(max_decimal.physical, PhysicalType::FixedLenByteArray);
    assert_eq!(max_decimal.type_length, 16);
    assert_eq!(max_decimal.precision, 38);
    assert_eq!(max_decimal.scale, 6);
    assert_eq!(
        max_decimal.logical,
        LogicalType::Decimal {
            precision: 38,
            scale: 6,
        }
    );
    assert_eq!(
        to_column_type(&info("DECIMAL", 0, 2)).logical,
        LogicalType::String
    );
    assert_eq!(
        to_column_type(&info("DECIMAL", 39, 2)).logical,
        LogicalType::String
    );
    let numeric_overflow = to_column_type(&info("NUMERIC", 39, 2));
    assert_eq!(numeric_overflow.physical, PhysicalType::ByteArray);
    assert_eq!(numeric_overflow.logical, LogicalType::None);
    // 宽度计算对非正精度返回哨兵值，并在最小有效精度处取整为 1 字节。
    assert_eq!(decimal_fixed_length_bytes_for_precision(0), -1);
    assert_eq!(decimal_fixed_length_bytes_for_precision(2), 1);
}
