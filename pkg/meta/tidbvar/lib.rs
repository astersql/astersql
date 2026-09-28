// Copyright 2026 AsterSQL.

// tidbvar crate 根模块：暴露写入 `mysql.tidb` 系统表的键名常量。
//
// `mysql.tidb` 存放集群级内部变量（非会话系统变量）；DXF（分布式执行框架）等组件
// 通过这些键协调调度与缩容，真正的键定义在 `vars` 子模块。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 变量键名定义（如 DXF 缩容暂停标志）。
pub mod vars;
pub use vars::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
