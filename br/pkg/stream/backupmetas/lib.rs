// Copyright 2026 AsterSQL.

//! 流式备份元数据解析包入口：导出 `parser`。
//! 对应 Go `br/pkg/stream/backupmetas`，解析 log backup 元文件。
//! 本文件只装配模块；解析逻辑在 parser。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
#[path = "parser.rs"]
pub mod parser;
pub use parser::*;
