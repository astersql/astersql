// Copyright 2026 AsterSQL.

//! BR checksum 子包 crate 根：聚合执行器实现、桩类型与测试模块。
//! 对应 Go `br/pkg/checksum` 包入口；对外重导出 executor 与 stubs 公共 API。
//! 模块装配顺序：先 stubs（类型/接口桩），再 executor（真实校验逻辑）。
//! 测试模块仅在 `cfg(test)` 下挂载，不进入生产依赖图。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

// 桩与适配层：TableInfo/Client 等依赖面，供 executor 与测试共用。
#[path = "stubs.rs"]
pub mod stubs;

// 校验执行器：Builder、请求展开与聚合，对齐 Go executor.go。
#[path = "executor.rs"]
pub mod executor;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "executor_nokit_test.rs"]
mod executor_nokit_test;

#[cfg(test)]
#[path = "executor_test.rs"]
mod executor_test;

// 扁平导出，使 `astersql_br_pkg_checksum::*` 与 Go 包级符号用法接近。
pub use executor::*;
pub use stubs::*;
