// Copyright 2026 AsterSQL.

// 优化器 fix-control 工具包入口。
//
// fix-control 是按 issue 编号配置的优化器开关字符串映射；本 crate 提供
// 编号常量与读取（`get`）、会话变量字符串解析（`set`），并在测试配置下
// 挂载对应单元测试模块。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 编号常量与按类型读取 API。
pub mod get;
/// 将 `key:value,...` 会话变量解析为 map。
pub mod set;
pub use get::*;
pub use set::*;

#[cfg(test)]
mod fixcontrol_test;
#[cfg(test)]
mod get_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod migration_aster_unit_test;
