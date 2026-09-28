// Copyright 2026 AsterSQL.

// SEM（Security Enhanced Mode，安全增强模式）v1 包入口。
//
// 本 crate 暴露 `sem` 模块中的 Enable/Disable 与可见性/权限查询 API，
// 并在 `cfg(test)` 下挂载迁移单元测试、main_test 与 sem_test。

mod sem;

pub use sem::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "sem_test.rs"]
mod sem_test;
