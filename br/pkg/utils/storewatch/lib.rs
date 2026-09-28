// Copyright 2026 AsterSQL.

//! PD/TiKV store 变更监视包入口：导出 `watching` 实现。
//! 对应 Go `br/pkg/utils/storewatch`，供流式备份等感知 store 上下线。
//! 测试与实现分文件挂载，根上扁平再导出公开 API。
//! 监视语义与重试策略见 `watching` 模块，此处只做 crate 边界。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
#[path = "watching.rs"]
pub mod watching;
#[cfg(test)]
#[path = "watching_test.rs"]
mod watching_test;
pub use watching::*;
