// Copyright 2026 AsterSQL.

// `parsedef` crate 入口：导出行数据定义，并挂接迁移单元测试。

/// 行结构与 ArrayEncoder 定义。
mod def;
/// 将 `def` 的公开项再导出到 crate 根。
pub use def::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
// 仅在测试构建下编译的迁移对照单元测试。
mod migration_aster_unit_test;
