// DXF Framework execute mock 子 crate 入口。
//
// 导出 `StepExecutor` 的 mockall 替身，并在测试配置下挂载迁移回归测试模块。
// Copyright 2026 AsterSQL.

// mockall 生成的 StepExecutor mock 实现。
mod execute_mock;
pub use execute_mock::*;

// 仅测试构建：迁移回归用例。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
