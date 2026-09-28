// Copyright 2026 AsterSQL.

//! Crate entry for `tools/gen-parquet` (Go package main — test parquet generator).
//!
//! 这个文件本身不承载 parquet 生成逻辑，而是把可执行程序的 `main.rs`
//! 包装成可复用的库入口，便于测试或其他调用方在不直接走进程启动的情况下复用同一实现。
//! 导出关系保持与 Go 的 `package main` 一致：核心行为仍由 `main::main()` 驱动，
//! `lib.rs` 只负责模块挂载、测试接线与统一入口转发。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::all
)]

#[path = "main.rs"]
/// 将二进制入口实现作为同名模块重新导出，避免库与可执行版本出现两套逻辑。
pub mod main;

#[cfg(test)]
#[path = "parity_test.rs"]
/// 测试仅在 test 配置下挂载，确保生产构建仍保持与 Go 工具相同的最小入口形态。
mod parity_test;

/// Binary / library process entry matching Go `main`.
/// 对外暴露单一入口，调用方无需关心底层文件布局，只需复用与命令行一致的执行路径。
pub fn entry() {
    main::main();
}
