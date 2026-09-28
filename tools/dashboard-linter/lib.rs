// Copyright 2026 AsterSQL.

//! Crate entry for `tools/dashboard-linter` (Go package main — Grafana dashboard JSON linter).
// 这个库文件本身不实现 lint 规则，而是把 `main.rs` 中的命令行流程包装成可复用的 crate 入口。
// 这样测试和其他 Rust 调用方可以复用同一套 Go 对齐逻辑，而不是复制一份独立的启动路径。
// 公开导出的 `main` 模块保留与 Go `main` 包接近的符号布局，便于机械迁移后的对照和回归验证。
// `entry()` 则把库形态重新收束为单一入口，供需要“像二进制一样执行”但又不能直接依赖 bin target 的场景调用。

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
pub mod main;

#[cfg(test)]
#[path = "parity_test.rs"]
// 对齐 Go 行为的回归测试只在测试构建中挂接，避免影响正常库入口的导出面。
mod parity_test;

/// Binary / library process entry matching Go `main`.
/// 这里不增加额外初始化，直接转发到 `main::main()`，确保库入口与命令行入口共享完全相同的退出码语义。
pub fn entry() {
    main::main();
}
