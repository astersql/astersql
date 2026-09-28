// Copyright 2026 AsterSQL.

// `util/naming` crate 入口：命名服务相关工具。
//
// 对应 Go `pkg/util/naming`。对外再导出 `naming` 模块；测试覆盖原命名
// 用例与 AsterSQL 迁移补充用例。

#![allow(non_snake_case, non_upper_case_globals)]

/// 命名服务核心实现模块。
pub mod naming;

pub use naming::*;

/// 对齐 Go 原命名相关测试。
#[cfg(test)]
mod naming_test;

/// AsterSQL 迁移补充的 naming 单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
