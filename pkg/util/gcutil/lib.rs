// Copyright 2026 AsterSQL.

// gcutil 包入口：TiDB GC（垃圾回收）安全点校验与启停辅助。
//
// 对外公开 `gcutil` 模块 API（读 `tikv_gc_safe_point`、校验 snapshot TS 等）；
// 测试配置下挂载迁移回归用例。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// GC 安全点与启停核心实现。
mod gcutil;
/// 重新导出 `gcutil` 中的公开类型与函数。
pub use gcutil::*;

/// 迁移期回归：安全点解析、snapshot 校验与 GC 启停。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
