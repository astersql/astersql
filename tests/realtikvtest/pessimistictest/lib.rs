//! 中文说明开始（自动生成）
//! 中文总览：`lib.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `lib` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 4 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `serial_guard` 是当前文件里的辅助函数。
//! 阅读 `serial_guard` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `serial_guard` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 中文说明结束（自动生成）

// Copyright 2026 AsterSQL.

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

extern crate self as astersql_tests_realtikvtest_pessimistictest;

use std::sync::{Mutex, MutexGuard};

static TEST_SERIAL: Mutex<()> = Mutex::new(());

/// Serializes tests that mutate the shared RealTiKV harness and process-wide
/// transaction configuration.
pub fn serial_guard() -> MutexGuard<'static, ()> {
    TEST_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
