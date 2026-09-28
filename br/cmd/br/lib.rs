// Copyright 2026 AsterSQL.

//! `br/cmd/br` 的 Rust 模块总入口。
//! 该文件本身几乎不承载业务逻辑，主要负责把各个命令模块、桩实现
//! 与测试模块组织成一个可被二进制包装层和测试复用的 crate 结构。
//! 这种拆分方式对应 Go 版本里 `package main` 下的多文件布局，
//! 让各子命令仍能共享同一套全局状态与公共 helper。

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
/// 集中放置对外部依赖的 slim stub，供命令层在迁移阶段复用。
pub mod stubs;

#[path = "cmd.rs"]
/// 公共 CLI 初始化、日志和 status server 逻辑。
pub mod cmd;

#[path = "abort.rs"]
/// `br abort` 命令族。
pub mod abort;

#[path = "backup.rs"]
/// `br backup` 命令族。
pub mod backup;

#[path = "debug.rs"]
/// 隐藏的 `br debug`/`validate` 工具命令。
pub mod debug;

#[path = "fips.rs"]
/// FIPS 能力查询适配层。
pub mod fips;

#[path = "main.rs"]
/// 二进制入口实现，单独命名为 `entry` 以避免与本文件导出的 `main()` 冲突。
pub mod entry;

#[path = "operator.rs"]
/// `br operator` 子命令。
pub mod operator;

#[path = "restore.rs"]
/// `br restore` 命令族。
pub mod restore;

#[path = "stream.rs"]
/// `br stream` 命令族。
pub mod stream;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "backup_test.rs"]
mod backup_test;

#[cfg(test)]
#[path = "cmd_test.rs"]
mod cmd_test;

#[cfg(test)]
#[path = "debug_test.rs"]
mod debug_test;

#[cfg(test)]
#[path = "stream_test.rs"]
mod stream_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// Shared process entrypoint called by the binary wrapper.
///
/// 中文补充：这里仅做一次薄转发，确保库模式与二进制模式共享同一入口实现。
pub fn main() {
    entry::main();
}
