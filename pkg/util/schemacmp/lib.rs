// Copyright 2026 AsterSQL.

// Schema 比较（schemacmp）crate 入口：用格（lattice）语义比较/合并表结构。
//
// 对应 Go `pkg/util/schemacmp`。核心思路是把列类型、字符集/排序规则、表定义
// 编码为偏序格上的元素，通过 `Compare`/`Join` 判断兼容性并求上确界（join），
// 用于 DDL 兼容性检查与 schema 合并。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 再导出 AST 与 MySQL 类型常量，供本 crate 与测试使用。
pub use meta_model::{ast, mysql};
/// 元数据模型命名空间（再导出 `meta_model`）。
pub mod model {
    pub use meta_model::*;
}
/// 字符集相关类型再导出。
pub mod charset {
    pub use parser_types::charset::*;
}
/// 字段类型与 `NewFieldTypeBuilder` 再导出。
pub mod types {
    pub use parser_types::types::*;
    pub use tidb_types::NewFieldTypeBuilder;
}
/// SQL 格式化工具再导出。
pub use parser_types::format;

/// 格代数核心：`Lattice` trait、基础格元素与不相容错误。
mod lattice;
pub use lattice::*;
/// 字符集/排序规则上的格实现。
mod charset_collation;
pub use charset_collation::*;
/// MySQL 整数/BLOB 类型编号的三态比较辅助。
mod util;
pub use util::{compareMySQLBlobType, compareMySQLIntegerType};
/// 列类型（`FieldType`）到格的编解码与 join。
mod r#type;
pub use r#type::*;
/// 表级 schema 的格表示与合并。
mod table;
pub use table::*;

/// 字符集/排序规则迁移补充单元测试。
#[cfg(test)]
#[path = "charset_collation_1_aster_unit_test.rs"]
mod charset_collation_1_aster_unit_test;
/// 字符集/排序规则单元测试。
#[cfg(test)]
#[path = "charset_collation_test.rs"]
mod charset_collation_test;
/// 格代数基础元素单元测试。
#[cfg(test)]
#[path = "lattice_test.rs"]
mod lattice_test;
/// 表 schema join 单元测试。
#[cfg(test)]
#[path = "table_test.rs"]
mod table_test;
/// 列类型格迁移补充单元测试。
#[cfg(test)]
#[path = "type_2_aster_unit_test.rs"]
mod type_2_aster_unit_test;
/// 列类型格单元测试。
#[cfg(test)]
#[path = "type_test.rs"]
mod type_test;
