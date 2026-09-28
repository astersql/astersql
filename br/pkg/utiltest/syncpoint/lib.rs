// Copyright 2026 AsterSQL.

//! 测试用 syncpoint 包入口：导出同步点实现与 Cancel/StopWatch stubs。
//! 对应 Go utiltest/syncpoint，用于在单测中注入可等待屏障。
//! `TEST_LOCK` 串行化依赖全局状态的用例，避免并行互相干扰。
//! stubs 提供 after_func 等时间钩子替身。
//! 实现细节见 `syncpoint` 模块；本文件仅装配边界。

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

#[path = "syncpoint.rs"]
pub mod syncpoint;

pub use stubs::{CancelHandle, Context, StopWatch, after_func};
pub use syncpoint::*;

#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "syncpoint_test.rs"]
mod syncpoint_test;
