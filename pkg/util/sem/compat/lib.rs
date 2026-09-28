// Copyright 2026 AsterSQL.

// SEM 兼容层 crate 入口。
//
// 聚合 `sem` 实现与 `testhelper`，并在测试配置下挂载 compat、
// 集成与迁移单元测试模块。SEM（Security Enhanced Mode）用于限制
// 敏感 schema/表/变量/权限的可见性。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// SEM 核心实现模块。
#[path = "sem.rs"]
mod sem;
/// 导出 SEM 公共 API。
pub use sem::*;

/// 测试辅助（切换 SEM 版本等）。
#[path = "testhelper.rs"]
mod testhelper;
/// 导出测试辅助 API。
pub use testhelper::*;

/// 兼容层可见性单元测试。
#[cfg(test)]
mod compat_test;
/// SEM 集成测试。
#[cfg(test)]
mod sem_integration_test;

/// 迁移对照测试：通过 include! 引入独立测试源文件。
#[cfg(test)]
mod migration_aster_unit_test {
    use super::*;
    use astersql_sessionctx_vardef as vardef;
    use astersql_sessionctx_variable as variable;

    include!("migration_aster_unit_test.rs");
}
