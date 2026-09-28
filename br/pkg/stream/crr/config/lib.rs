// Copyright 2026 AsterSQL.

//! CRR 流式检查点服务的配置包入口，对齐 Go `br/pkg/stream/crr/config`。
//! 职责：挂载 `config` 实现模块并扁平再导出公开 API，供 CLI/服务组装默认值与 flag。
//! 初始化顺序：`config` 模块先声明，再 `pub use`；测试模块仅在 `cfg(test)` 下按 path 挂载。
//! 不在此文件定义业务逻辑，避免与 Go 单包多文件布局错位。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

// 显式 path 固定实现文件，便于与 Go 包目录一一对照。
#[path = "config.rs"]
pub mod config;

// 对外扁平导出 Config/DefaultConfig/DefineFlags 等，调用方不必写 config::config::。
pub use config::*;

// Go/Rust 公共契约对照：默认值与 Parse 覆盖语义。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

// 单元测试（对应 Go config_test.go），与实现分文件以符合仓库约定。
#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;
