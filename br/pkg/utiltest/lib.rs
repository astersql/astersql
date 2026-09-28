// Copyright 2026 AsterSQL.

//! BR 测试工具包入口：导出 stubs 存储抽象与 restore schema suite。
//! 对应 Go `br/pkg/utiltest`，供 restore/schema 等单测搭建本地存储夹具。
//! stubs 提供 Storage/Reader/Writer；suite 封装 RestoreSchemaSuite 工厂。
//! 本文件只装配模块边界，不包含具体用例逻辑。
//! parity_test 校验与 Go 公开契约一致。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

#[path = "stubs.rs"]
pub mod stubs;

#[path = "suite.rs"]
pub mod suite;

pub use stubs::{
    Context, Error, LocalStorage, NewLocalStorage, Reader, ReaderOption, Result, Storage,
    WalkOption, Writer, WriterOption,
};
pub use suite::{CreateRestoreSchemaSuite, RestoreSchemaSuite, TestRestoreSchemaSuite};

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
