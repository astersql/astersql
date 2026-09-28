// Copyright 2026 AsterSQL.

// `dbutiltest` 测试辅助 crate 入口。
//
// 导出 `utils` 中的建表元数据工具，并在测试配置下挂载 `utils_aster_unit_test`。

#![allow(dead_code)]

/// 测试工具实现模块（含 `GetTableInfoBySQL`）。
mod utils;
/// 将 utils 公共 API 再导出到 crate 根。
pub use utils::*;

#[cfg(test)]
#[path = "utils_aster_unit_test.rs"]
/// Aster 侧单元测试：验证建表 SQL 到 TableInfo 的元数据构建。
mod utils_aster_unit_test;
