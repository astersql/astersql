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

// database/sql 列类型名 → Parquet 物理/逻辑类型映射。
//
// 将 MySQL driver 报告的 `DatabaseTypeName`（如 TIMESTAMP、DECIMAL）转为
// Parquet ColumnType。未识别或别名类型降级为 BYTE_ARRAY，保持前向兼容。
// 文件前半为 Go 草稿注释，后半为可编译实现。

// database/sql 列类型名如何映射为 Parquet physical/logical type。
//
// toColumnType converts database type to parquet column type.
// ColumnInfo.DatabaseTypeName must come from database/sql
// ColumnType.DatabaseTypeName().
// With MySQL drivers, temporal types are reported as base names
// (e.g. TIMESTAMP/DATETIME) without precision suffixes such as "(6)".
// go-sql-driver/mysql DatabaseTypeName() does not emit the type aliases below.
// They are intentionally not specialized and fall back to BYTE_ARRAY so unknown
// or driver-specific names remain forward compatible with string-like export.
// NUMERIC, FIXED, UNSIGNED MEDIUMINT, VECTOR, NCHAR, NVARCHAR, CHARACTER,
// VARCHARACTER, SQL_TSI_YEAR, VAR_STRING, LONG, INTEGER, INT1, INT2, INT3, INT8,
// BOOL, BOOLEAN, REAL, DOUBLE PRECISION,
// toColumnType 对应 Go 的类型映射入口：先清理并大写数据库类型名，再走同一 switch 分支。
// pub fn toColumnType(columnInfo: &ColumnInfo) -> columnType {
//     let dbTypeName = strings::ToUpper(strings::TrimSpace(&columnInfo.DatabaseTypeName));
//     match dbTypeName.as_str() {
//         "CHAR" | "VARCHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT"
//         | "DATE" | "TIME" | "SET" | "JSON" | "ENUM" | "NULL" | "GEOMETRY" => {
// "NULL" 表示 MySQL 协议层返回 MYSQL_TYPE_NULL，例如 SELECT NULL AS c。
//             columnType {
//                 Physical: parquet::Types::ByteArray,
//                 Logical: schema::StringLogicalType {},
//                 TypeLength: -1,
//                 Precision: 0,
//                 Scale: 0,
//             }
//         }
//         "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" | "BINARY" | "VARBINARY" | "BIT" => {
//             columnType {
//                 Physical: parquet::Types::ByteArray,
//                 Logical: schema::LogicalType::None,
//                 TypeLength: -1,
//                 Precision: 0,
//                 Scale: 0,
//             }
//         }
//         "TIMESTAMP" | "DATETIME" => columnType {
// Go 使用 isAdjustedToUTC=false 的 micros timestamp，保留“本地语义按 UTC 数值编码”的约定。
//             Physical: parquet::Types::Int64,
//             Logical: schema::NewTimestampLogicalType(false, schema::TimeUnitMicros),
//             TypeLength: -1,
//             Precision: 0,
//             Scale: 0,
//         },
//         "YEAR" | "TINYINT" | "SMALLINT" | "MEDIUMINT" | "UNSIGNED TINYINT"
//         | "UNSIGNED SMALLINT" | "INT" => {
// Go 注释说明 UNSIGNED MEDIUMINT 会由 driver 报为 MEDIUMINT，INT32 足以承载。
//             columnType {
//                 Physical: parquet::Types::Int32,
//                 Logical: schema::LogicalType::None,
//                 TypeLength: -1,
//                 Precision: 0,
//                 Scale: 0,
//             }
//         }
//         "BIGINT" | "UNSIGNED INT" => columnType {
//             Physical: parquet::Types::Int64,
//             Logical: schema::LogicalType::None,
//             TypeLength: -1,
//             Precision: 0,
//             Scale: 0,
//         },
//         "DECIMAL" => decimalColumnType(columnInfo),
//         "UNSIGNED BIGINT" => columnType {
// UNSIGNED BIGINT 最大 20 位十进制数，Go 映射为 DECIMAL(20,0) 的 fixed byte array。
//             Physical: parquet::Types::FixedLenByteArray,
//             Logical: schema::NewDecimalLogicalType(20, 0),
//             TypeLength: 9,
//             Precision: 20,
//             Scale: 0,
//         },
//         "FLOAT" => columnType {
// MySQL FLOAT 存储为 4 字节，但表达式求值可能需要 double 精度；Go 为兼容性写 DOUBLE。
//             Physical: parquet::Types::Double,
//             Logical: schema::LogicalType::None,
//             TypeLength: -1,
//             Precision: 0,
//             Scale: 0,
//         },
//         "DOUBLE" => columnType {
//             Physical: parquet::Types::Double,
//             Logical: schema::LogicalType::None,
//             TypeLength: -1,
//             Precision: 0,
//             Scale: 0,
//         },
//         _ => columnType {
// 未识别类型统一降级为 BYTE_ARRAY，延续 Go 的 forward compatible 策略。
//             Physical: parquet::Types::ByteArray,
//             Logical: schema::LogicalType::None,
//             TypeLength: -1,
//             Precision: 0,
//             Scale: 0,
//         },
//     }
// }
//
// decimalColumnType 对应 Go 的 DECIMAL 专用映射。
// precision 超出常见下游 38 位上限时写成 UTF-8 字符串，避免 reader 兼容性问题。
// pub fn decimalColumnType(columnInfo: &ColumnInfo) -> columnType {
//     let precision = columnInfo.Precision as i32;
//     let scale = columnInfo.Scale as i32;
//     if precision <= 0 || precision > 38 {
//         return columnType {
//             Physical: parquet::Types::ByteArray,
//             Logical: schema::StringLogicalType {},
//             TypeLength: -1,
//             Precision: 0,
//             Scale: 0,
//         };
//     }
//
//     let decimalLogicalType = schema::NewDecimalLogicalType(precision, scale);
//     if precision <= 9 {
//         return columnType {
//             Physical: parquet::Types::Int32,
//             Logical: decimalLogicalType,
//             TypeLength: -1,
//             Precision: precision,
//             Scale: scale,
//         };
//     }
//     if precision <= 18 {
//         return columnType {
//             Physical: parquet::Types::Int64,
//             Logical: decimalLogicalType,
//             TypeLength: -1,
//             Precision: precision,
//             Scale: scale,
//         };
//     }
//
//     let typeLength = decimalFixedLengthBytesForPrecision(precision);
//     columnType {
//         Physical: parquet::Types::FixedLenByteArray,
//         Logical: decimalLogicalType,
//         TypeLength: typeLength,
//         Precision: precision,
//         Scale: scale,
//     }
// }
//
// decimalFixedLengthBytesForPrecision 对应 Go 的数学换算。
// Parquet DECIMAL 的 FIXED_LEN_BYTE_ARRAY 需要 precision <= floor(log10(2^(8*n - 1) - 1))。
// pub fn decimalFixedLengthBytesForPrecision(precision: i32) -> i32 {
//     if precision <= 0 {
//         return -1;
//     }
// Go 公式为 ceil((precision*log2(10)+1)/8)，这里保持同一推导和取整方向。
//     math::Ceil(((precision as f64) * math::Log2(10.0) + 1.0) / 8.0) as i32
// }
// */
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Parquet 物理存储类型。
pub enum PhysicalType {
    Boolean,
    Int32,
    Int64,
    Float,
    Double,
    Int96,
    ByteArray,
    FixedLenByteArray,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 时间戳/时间逻辑类型的时间单位。
pub enum TimeUnit {
    Millis,
    Micros,
    Nanos,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// Parquet 逻辑类型（在物理类型之上的语义注解）。
pub enum LogicalType {
    /// 无逻辑类型。
    None,
    /// UTF-8 字符串。
    String,
    /// 定点数：precision 总位数，scale 小数位数。
    Decimal { precision: i32, scale: i32 },
    /// 时间戳；`adjusted_to_utc` 表示是否按 UTC 调整。
    Timestamp {
        adjusted_to_utc: bool,
        unit: TimeUnit,
    },
    /// 日期。
    Date,
    /// 日内时间。
    Time {
        adjusted_to_utc: bool,
        unit: TimeUnit,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// 完整的 Parquet 列类型描述。
pub struct ColumnType {
    /// 物理类型。
    pub physical: PhysicalType,
    /// 逻辑类型。
    pub logical: LogicalType,
    /// FIXED_LEN_BYTE_ARRAY 宽度；其它类型通常为 -1。
    pub type_length: i32,
    /// DECIMAL 精度（其它类型多为 0）。
    pub precision: i32,
    /// DECIMAL 小数位。
    pub scale: i32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// 来自 database/sql 的列元信息。
pub struct ColumnInfo {
    /// 列名。
    pub name: String,
    /// driver 报告的类型名（如 `VARCHAR`、`UNSIGNED BIGINT`）。
    pub database_type_name: String,
    /// 是否可空。
    pub nullable: bool,
    /// 精度（DECIMAL 等）。
    pub precision: i32,
    /// 小数位。
    pub scale: i32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// 导出用的列描述：元信息 + 映射后的类型 + null 编码标志。
pub struct Column {
    /// 原始列信息。
    pub info: ColumnInfo,
    /// 映射得到的 Parquet 类型。
    pub column_type: ColumnType,
    /// 是否需要 definition level 空值编码。
    pub allows_null_encoding: bool,
    /// 时间戳写入单位。
    pub timestamp_unit: TimeUnit,
}

/// 构造无逻辑类型、type_length=-1 的简单 ColumnType。
fn plain(physical: PhysicalType) -> ColumnType {
    ColumnType {
        physical,
        logical: LogicalType::None,
        type_length: -1,
        precision: 0,
        scale: 0,
    }
}
/// 将 DatabaseTypeName 映射为 Parquet ColumnType（先 trim+大写）。
pub fn to_column_type(info: &ColumnInfo) -> ColumnType {
    // 与 Go toColumnType 同一组分支：字符/BLOB/时间/整数/DECIMAL/浮点/兜底。
    let name = info.database_type_name.trim().to_ascii_uppercase();
    match name.as_str() {
        "CHAR" | "VARCHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT" | "DATE" | "TIME"
        | "SET" | "JSON" | "ENUM" | "NULL" | "GEOMETRY" => ColumnType {
            logical: LogicalType::String,
            ..plain(PhysicalType::ByteArray)
        },
        "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" | "BINARY" | "VARBINARY" | "BIT" => {
            plain(PhysicalType::ByteArray)
        }
        // 微秒时间戳，isAdjustedToUTC=false（本地语义按 UTC 数值编码）。
        "TIMESTAMP" | "DATETIME" => ColumnType {
            physical: PhysicalType::Int64,
            logical: LogicalType::Timestamp {
                adjusted_to_utc: false,
                unit: TimeUnit::Micros,
            },
            type_length: -1,
            precision: 0,
            scale: 0,
        },
        "YEAR" | "TINYINT" | "SMALLINT" | "MEDIUMINT" | "UNSIGNED TINYINT"
        | "UNSIGNED SMALLINT" | "INT" => plain(PhysicalType::Int32),
        "BIGINT" | "UNSIGNED INT" => plain(PhysicalType::Int64),
        "DECIMAL" => decimal_column_type(info),
        // 最大 20 位十进制，映射为 DECIMAL(20,0) 的 9 字节 fixed array。
        "UNSIGNED BIGINT" => ColumnType {
            physical: PhysicalType::FixedLenByteArray,
            logical: LogicalType::Decimal {
                precision: 20,
                scale: 0,
            },
            type_length: 9,
            precision: 20,
            scale: 0,
        },
        // MySQL FLOAT 也写 DOUBLE，兼容表达式求值精度。
        "FLOAT" | "DOUBLE" => plain(PhysicalType::Double),
        // 未识别类型降级 BYTE_ARRAY。
        _ => plain(PhysicalType::ByteArray),
    }
}
/// DECIMAL 专用：精度≤9→Int32，≤18→Int64，≤38→FixedLen；超出则字符串。
pub fn decimal_column_type(info: &ColumnInfo) -> ColumnType {
    let (p, s) = (info.precision, info.scale);
    // 超出常见下游 38 位上限时写 UTF-8 字符串，避免 reader 不兼容。
    if p <= 0 || p > 38 {
        return ColumnType {
            logical: LogicalType::String,
            ..plain(PhysicalType::ByteArray)
        };
    }
    let physical = if p <= 9 {
        PhysicalType::Int32
    } else if p <= 18 {
        PhysicalType::Int64
    } else {
        PhysicalType::FixedLenByteArray
    };
    ColumnType {
        physical,
        logical: LogicalType::Decimal {
            precision: p,
            scale: s,
        },
        type_length: if p > 18 {
            decimal_fixed_length_bytes_for_precision(p)
        } else {
            -1
        },
        precision: p,
        scale: s,
    }
}
/// 由精度推算 FIXED_LEN_BYTE_ARRAY 字节数：ceil((p*log2(10)+1)/8)。
pub fn decimal_fixed_length_bytes_for_precision(precision: i32) -> i32 {
    if precision <= 0 {
        -1
    } else {
        ((precision as f64 * std::f64::consts::LOG2_10 + 1.0) / 8.0).ceil() as i32
    }
}
/// Go 风格别名。
pub fn toColumnType(info: &ColumnInfo) -> ColumnType {
    to_column_type(info)
}
/// Go 风格别名。
pub fn decimalColumnType(info: &ColumnInfo) -> ColumnType {
    decimal_column_type(info)
}
/// Go 风格别名。
pub fn decimalFixedLengthBytesForPrecision(p: i32) -> i32 {
    decimal_fixed_length_bytes_for_precision(p)
}
