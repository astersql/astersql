// Copyright 2026 AsterSQL.

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

//! BR glue crate 入口：汇总控制台、抽象门面与进度实现，并对齐 Go `br/pkg/glue` 包边界。
//! 模块用 `#[path]` 显式挂接同目录源文件，便于与 Go 单文件布局对照维护。
//! 测试模块仅在 `cfg(test)` 下编译，避免生产产物携带 parity / console 用例。

#[path = "console_glue.rs"]
pub mod console_glue;

#[path = "glue.rs"]
pub mod glue;

#[path = "progressing.rs"]
pub mod progressing;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "console_glue_test.rs"]
mod console_glue_test;

// 对外再导出：调用方可用 `astersql_br_glue::*` 直接拿到门面与进度类型。
pub use console_glue::*;
pub use glue::*;
// 进度子系统只导出稳定公开符号，内部 BarState 等保持私有。
pub use progressing::{
    LogBar, MultiProgress, NopMultiProgress, OnlyOneTask, ProgressBar, ProgressWaiter, TerminalBar,
    TerminalMultiProgress,
};
