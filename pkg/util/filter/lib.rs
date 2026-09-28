// Copyright 2026 AsterSQL.

// `util/filter` crate 入口：按 MySQL 复制规则风格过滤库表。
//
// 对应 Go `util/filter`。复制过滤（replication filter）用 DoDB/IgnoreDB、
// DoTable/IgnoreTable 等规则决定哪些 schema/table 需要同步或忽略；
// 本 crate 导出 Filter 与 schema 相关辅助。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

extern crate self as util_filter;

/// 表过滤器核心实现（规则初始化、匹配与缓存）。
pub mod filter;
/// schema 名与通配/正则相关辅助。
pub mod schema;
pub use filter::*;
pub use schema::*;

#[cfg(test)]
#[path = "filter_test.rs"]
/// Filter 表驱动与大小写相关单元测试。
mod filter_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移对照的 Aster 单元测试。
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "schema_test.rs"]
/// schema 辅助逻辑单元测试。
mod schema_test;
