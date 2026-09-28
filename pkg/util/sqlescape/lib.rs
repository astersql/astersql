// Copyright 2026 AsterSQL.

// SQL 字符串转义与格式化工具 crate 入口。
//
// 对应 Go `pkg/util/sqlescape`：`utils` 实现 `%?`/`%n`/`%%` 占位符转义；
// 本文件再导出公开 API，并通过 `#[path]` 挂载迁移回归测试。

#![allow(non_snake_case, dead_code)]

/// SQL 转义与格式化实现模块。
pub mod utils;
/// 对外再导出 `utils` 中的公开类型与函数。
pub use utils::*;

/// 迁移期单元测试：占位符、数值/时间/二进制路径与 Must 辅助函数。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
