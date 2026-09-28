// Copyright 2026 AsterSQL.

//! Crate entry for `dumpling/context` (Go package `github.com/pingcap/tidb/dumpling/context`).
//!
//! crate 对外主入口是 dumpling 自己的 `Context` 包装层，
//! 同时额外导出带 `Go` 前缀的 stub 类型，方便需要直接对照 Go 语义的测试使用。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

#[path = "stubs.rs"]
// 这里的 stub 对应 Go 标准库 context 的最小子集，而不是 dumpling 自定义包装层。
mod stubs;

#[path = "context.rs"]
mod context;

pub use context::*;
// `Go*` 前缀用于显式区分“标准库风格 context stub”和“dumpling 包装 context”。
pub use stubs::{Background as GoBackground, Context as GoContext, WithCancel as GoWithCancel};
pub use stubs::{CancelFunc, Canceled};

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
