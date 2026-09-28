// Copyright 2026 AsterSQL.

//! BR 版本检查包入口：导出 `version` 模块的集群/BR 版本兼容逻辑。
//! 对应 Go `br/pkg/version`；备份恢复前校验 TiKV/PD/TiDB 版本约束。
//! 单元测试与 parity 测试经 path 挂载到本 crate。
//! 具体比较与错误文案在 `version` 模块，此处仅装配导出。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

#[path = "version.rs"]
pub mod version;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "version_test.rs"]
mod version_test;

pub use version::*;
