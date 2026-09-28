// Copyright 2026 AsterSQL.
// 表/列过滤器 crate 入口：导出匹配器、解析器、兼容层与 Filter 接口。
//
// 用于按规则挑选要同步或处理的 schema.table（及列），常见于数据迁移与复制过滤。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 列过滤器：按列名规则匹配。
pub mod column_filter;
/// MySQL 复制规则兼容转换。
pub mod compat;
/// 字符串/通配/正则匹配器与规则结构。
pub mod matchers;
/// 过滤规则文本解析与 `@file` 导入。
pub mod parser;
/// 表 Filter 实现与 Parse/All/CaseInsensitive 入口。
pub mod table_filter;

pub use column_filter::*;
pub use compat::*;
pub use matchers::*;
pub use parser::*;
pub use table_filter::*;

#[cfg(test)]
#[path = "column_filter_test.rs"]
mod column_filter_test;
#[cfg(test)]
#[path = "compat_test.rs"]
mod compat_test;
#[cfg(test)]
#[path = "matchers_test.rs"]
mod matchers_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "parser_test.rs"]
mod parser_test;
#[cfg(test)]
#[path = "table_filter_test.rs"]
mod table_filter_test;
