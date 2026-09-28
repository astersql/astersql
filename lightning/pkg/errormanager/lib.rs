// Copyright 2026 AsterSQL.

//! Crate entry for `lightning/pkg/errormanager`
//! (Go package `github.com/pingcap/tidb/lightning/pkg/errormanager`).
//!
//! 中文概览：这个入口把 Lightning 的导入错误管理子系统拆成真实实现、依赖桩和测试三层。
//! `errormanager.rs` 是主体，负责错误表初始化、阈值递减、冲突记录和错误汇总输出。
//! `stubs.rs` 则补齐当前迁移阶段尚未接上的 SQL、KV、日志、编码器等外围能力。
//! crate 根统一 `pub use`，让上层仍能按 Go 包级 API 方式引用错误管理器。
//! 测试被拆成公共契约回归、主体逻辑测试和 replace 冲突测试三类，
//! 这样既能保护导出面，也能分别覆盖普通错误记录和冲突替换分支。
//! 阅读顺序建议先看这里的导出关系，再看 `errormanager.rs` 的状态与 SQL 常量，
//! 最后进入两个测试文件理解阈值、冲突删除和输出格式为何必须与 Go 保持一致。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    clippy::all
)]

#[path = "stubs.rs"]
mod stubs;
// 先导出外部依赖桩，保证主体实现里的日志、SQL 和编码接口都有稳定占位边界。
pub use stubs::*;

#[path = "errormanager.rs"]
mod errormanager;
// 对外公开真实错误管理逻辑；调用方应把这里视为包级主入口。
pub use errormanager::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "errormanager_test.rs"]
mod errormanager_test;

#[cfg(test)]
#[path = "resolveconflict_test.rs"]
mod resolveconflict_test;
