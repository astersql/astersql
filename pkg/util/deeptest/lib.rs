// Copyright 2026 AsterSQL.

// Deep-clone / 深度克隆断言工具 crate 入口。
//
// 对应 Go 侧基于 `reflect` 的静态测试助手：因 Rust 无通用运行时反射，
// 调用方用 `DeepValue` 描述值形状，再做深拷贝相等或递归不等断言。
// 本文件仅导出 `statictesthelper` 并挂载相关单元测试。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 深度值描述与断言实现（对应 Go `statictesthelper`）。
pub mod statictesthelper;
pub use statictesthelper::*;

#[cfg(test)]
#[path = "statictesthelper_test.rs"]
/// 对应 Go `statictesthelper` 包内原有单元测试。
mod statictesthelper_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充回归测试。
mod migration_aster_unit_test;
