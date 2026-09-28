//! 中文说明开始（自动生成）
//! 中文总览：`lib.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `lib` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 4 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `stubs` 是当前文件里的模块。
//! 阅读 `stubs` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `stubs` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `testkit` 是当前文件里的模块。
//! 阅读 `testkit` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `testkit` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `parity_test` 是当前文件里的模块。
//! 阅读 `parity_test` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `parity_test` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 中文说明结束（自动生成）

// Copyright 2026 AsterSQL.

//! Crate entry for `tests/realtikvtest`
//! (Go package `github.com/pingcap/tidb/tests/realtikvtest`).

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
pub mod stubs;

#[path = "testkit.rs"]
mod testkit;

pub use testkit::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
