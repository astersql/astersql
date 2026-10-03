// Parquet 文件读写辅助 crate 的根模块。
//
// 提供 SQL 列类型映射、列值编解码、导入解析器、对象存储 Reader 包装、
// schema 构建、Spark legacy 历法 rebasing 与类型到 Datum 的转换。
// Copyright 2026 AsterSQL.

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]

/// 按物理类型准备的列写入缓冲。
pub mod column_buffer;
/// database/sql 类型名到 Parquet physical/logical type 的映射。
pub mod column_type;
/// RawBytes 解析、内存估算与列缓冲追加。
pub mod column_value;
/// 按 row group 读取 Parquet 并估算内存的导入解析器。
pub mod parser;
/// 对象存储 ReaderAt/Seek 包装与小 row group 预读。
pub mod reader_wrapper;
/// 由 ColumnInfo 构建 Parquet schema 节点。
pub mod schema_builder;
/// Spark Julian/Gregorian legacy DATE/TIMESTAMP rebasing。
pub mod spark_rebase;
/// Spark rebase 生成表（switches/diffs 与时区索引）。
pub mod spark_rebase_micros_generated;
/// Parquet 原始值到内部 Datum 的转换。
pub mod type_converter;
/// Parquet 写出：行缓冲、row group flush 与压缩。
pub mod writer;

/// 重新导出常用列类型元数据。
pub use column_type::{Column, ColumnInfo, ColumnType, LogicalType, PhysicalType, TimeUnit};
/// 重新导出列值枚举。
pub use column_value::ColumnValue;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 本 crate 统一错误类型（字符串包装）。
pub struct Error(pub String);
/// 按内部字符串展示错误。
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
/// 标准 Error trait 空实现。
impl std::error::Error for Error {}
/// IO 错误转为本 crate Error。
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self(value.to_string())
    }
}
/// 本 crate 统一 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
#[path = "benchmark_decimal_test.rs"]
/// DECIMAL 相关基准测试。
mod benchmark_decimal_test;
#[cfg(test)]
#[path = "column_type_mapping_test.rs"]
/// 列类型映射单测。
mod column_type_mapping_test;
#[cfg(test)]
#[path = "column_value_conversion_test.rs"]
/// 列值转换单测。
mod column_value_conversion_test;
#[cfg(test)]
#[path = "column_value_test.rs"]
/// 列值 Go 语义对齐回归测试。
mod column_value_test;
#[cfg(test)]
#[path = "parser_test.rs"]
/// 解析器单测。
mod parser_test;
#[cfg(test)]
#[path = "reader_wrapper_test.rs"]
/// Reader wrapper 与 row group 范围 Go 语义对齐回归测试。
mod reader_wrapper_test;
#[cfg(test)]
#[path = "schema_builder_test.rs"]
/// Schema 构建 Go 语义对齐回归测试。
mod schema_builder_test;
#[cfg(test)]
#[path = "spark_rebase_test.rs"]
/// Spark legacy 历法 rebasing Go 语义对齐回归测试。
mod spark_rebase_test;
#[cfg(test)]
#[path = "type_converter_test.rs"]
/// Parquet 类型转换 Go 语义对齐回归测试。
mod type_converter_test;
#[cfg(test)]
#[path = "writer_behavior_test.rs"]
/// Writer 行为单测。
mod writer_behavior_test;
#[cfg(test)]
#[path = "writer_core_test.rs"]
/// Writer 核心路径单测。
mod writer_core_test;
#[cfg(test)]
#[path = "writer_test.rs"]
/// Writer Go 语义对齐回归测试。
mod writer_test;
#[cfg(test)]
#[path = "writer_test_helpers_test.rs"]
/// Writer 测试辅助。
mod writer_test_helpers_test;

/// Object-store reader strategies shared by real Parquet column decoders.
pub mod source_reader;

/// Real column decoder for importer and sampling consumers.
pub mod file_parser;
