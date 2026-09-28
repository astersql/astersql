// Copyright 2026 AsterSQL.

// TopSQL collector mock 库入口。
//
// 导出测试用的 mock 收集器实现，并在测试配置下挂载迁移基线单元测试模块。

mod mock;
pub use mock::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
