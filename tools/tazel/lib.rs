// Copyright 2026 AsterSQL.

//! Crate entry for `tools/tazel` (Go package main — BUILD.bazel go_test patcher).
//! 该文件只负责把分散在同目录的实现重新导出成一个 crate 入口，
//! 让二进制入口、测试和库调用都复用同一套 Go 对齐逻辑。
//! 模块顺序也对应运行时依赖：先提供 BUILD 解析桩，再暴露 AST 统计、
//! 跳过规则与最终入口，避免调用方直接关心具体文件拆分。

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

#[path = "stubs.rs"]
/// BUILD 解析和改写的最小桩，实现与 Go buildtools API 的对接面。
pub mod stubs;

#[path = "ast.rs"]
/// 收集测试数等 AST/目录扫描信息，供 `main` 计算 `shard_count` 时复用。
pub mod ast;

#[path = "util.rs"]
/// 汇总路径过滤与写回辅助函数，保持与 Go 跳过策略一致。
pub mod util;

#[path = "main.rs"]
/// 真正的补丁流程入口，遍历仓库并修改 `BUILD.bazel` 中的 `go_test` 规则。
pub mod main;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "ast_test.rs"]
mod ast_test;

/// Binary / library process entry matching Go `main`.
/// 这里不复制启动逻辑，只转发到 `main::main()`，
/// 让库调用和二进制包装共享完全一致的行为。
pub fn entry() {
    main::main();
}
