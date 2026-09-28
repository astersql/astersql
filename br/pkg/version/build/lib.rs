// Copyright 2026 AsterSQL.

//! BR 构建信息子包入口：导出 `info`（版本/构建元数据日志等）。
//! 对齐 Go `br/pkg/version/build`；CLI 启动时 `LogInfo` 依赖此模块。
//! 测试经 `#[path]` 挂载，不与实现同文件混编。
//! 构建常量与格式化逻辑在 `info`，本文件只负责模块装配。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

#[path = "info.rs"]
pub mod info;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "info_test.rs"]
mod info_test;

pub use info::*;
