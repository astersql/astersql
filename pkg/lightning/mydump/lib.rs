// Copyright 2026 AsterSQL.

// Lightning mydump 子 crate：解析 mydumper/CSV 导出并组织为可导入的库表元数据。
//
// 模块覆盖字节集合、字符集转换、文件路由、分块读写、SQL/CSV 行解析、
// 目录加载（loader）、Region 切分、视图与 schema 导入。测试通过 `#[path]` 挂载，
// `test_support` 提供内存 Storage 供嵌入方与单测复用。

#![allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    dead_code
)]

mod common;
pub use common::*;
mod bytes;
pub use bytes::*;
mod charset_convertor;
pub use charset_convertor::*;
mod router;
pub use router::*;
mod reader;
pub use reader::*;
mod parser;
pub use parser::*;
mod csv_parser;
mod parser_generated;
pub use csv_parser::*;
mod loader;
pub use loader::*;
mod region;
pub use region::*;
mod view_import;
pub use view_import::*;
mod schema_import;
pub use schema_import::*;

#[cfg(test)]
#[path = "charset_convertor_test.rs"]
mod charset_convertor_test;
#[cfg(test)]
#[path = "csv_parser_test.rs"]
mod csv_parser_test;
#[cfg(test)]
#[path = "loader_test.rs"]
mod loader_test;
#[cfg(test)]
#[path = "parser_generated_test.rs"]
mod parser_generated_test;
#[cfg(test)]
#[path = "parser_test.rs"]
mod parser_test;
#[cfg(test)]
#[path = "reader_test.rs"]
mod reader_test;
#[cfg(test)]
#[path = "region_test.rs"]
mod region_test;
#[cfg(test)]
#[path = "router_test.rs"]
mod router_test;
#[cfg(test)]
#[path = "schema_import_test.rs"]
mod schema_import_test;
/// In-memory storage and file metadata builders shared by embedders and tests.
/// Keeping this adapter in the production module graph lets callers exercise
/// the same `Storage` contract without replacing loader logic.
///
/// 内存 Storage 与文件元数据构造器，供嵌入方与测试共用同一 Storage 契约。
pub mod test_support;
#[cfg(test)]
#[path = "view_import_test.rs"]
mod view_import_test;
