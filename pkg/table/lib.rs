// Copyright 2026 AsterSQL.

// table crate 的库入口。
//
// 聚合列（column）、CHECK 约束（constraint）、索引（index）与表契约（table）
// 等可执行边界，并向外再导出公共 API；`table.rs` 通过 `#[path]` 挂载为
// `table_impl`，避免与 crate 名冲突。测试模块同样用 `#[path]` 按 Go 包测试拆分挂载。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

extern crate self as table;

/// 列元数据与可执行默认/生成表达式状态。
pub mod column;
/// CHECK 约束元数据与校验辅助。
pub mod constraint;
/// 索引契约与索引 KV 生成器。
pub mod index;
/// 表契约实现（对应 Go `pkg/table/table.go`），路径别名避免与 crate 名冲突。
#[path = "table.rs"]
pub mod table_impl;

/// 再导出列相关公共类型与函数。
pub use column::*;
/// 再导出约束相关公共类型与函数。
pub use constraint::*;
/// 再导出索引相关公共类型与函数。
pub use index::*;
/// 再导出表契约相关公共类型与函数。
pub use table_impl::*;

/// Aster 迁移单元测试：列值转换、查找辅助等。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// 列模块的常规单元测试。
#[cfg(test)]
#[path = "column_test.rs"]
mod column_test;

/// 约束模块的 Aster 迁移单元测试。
#[cfg(test)]
#[path = "constraint_migration_aster_unit_test.rs"]
mod constraint_migration_aster_unit_test;

/// CHECK constraint Go/Rust parity regression tests.
#[cfg(test)]
#[path = "constraint_test.rs"]
mod constraint_test;

/// 索引模块的 Aster 迁移单元测试。
#[cfg(test)]
#[path = "index_migration_aster_unit_test.rs"]
mod index_migration_aster_unit_test;

/// 表契约模块的 Aster 迁移单元测试。
#[cfg(test)]
#[path = "table_migration_aster_unit_test.rs"]
mod table_migration_aster_unit_test;

/// 表错误码与变更选项的常规单元测试。
#[cfg(test)]
#[path = "table_test.rs"]
mod table_test;
