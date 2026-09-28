// Copyright 2026 AsterSQL.

//! mocklocal crate 入口：装配 stubs（类型替身）与 local（MockGen 移植），
//! 再导出供 ingest 控制面单测使用；parity_test 校验与 Go mocklocal 契约一致。
//! 允许 Go 风格命名与 clippy 噪音，避免机械翻译符号与上游不一致。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

// 本地 EngineFileSize / Range / Codec 与 take_ts 解包（无 kvproto/grpcio）。
#[path = "stubs.rs"]
pub mod stubs;

// MockDiskUsage / MockTiKVModeSwitcher / MockStoreHelper（对齐 local.go）。
#[path = "local.rs"]
pub mod local;

// 平铺导出 mock 类型与桩类型，调用方无需区分子模块路径。
pub use local::*;
pub use stubs::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
