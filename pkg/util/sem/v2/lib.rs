// Copyright 2026 AsterSQL.

// SEM（Security Enhanced Mode，安全增强模式）v2 crate 入口。
//
// 聚合配置解析（`config`）、运行时策略查询（`sem`）、SQL 限制规则（`sql_rule`）、
// 优化器 hint 限制（`restricted_hint`）与测试辅助（`testhelper`），
// 对外 re-export 公共 API。测试在 `cfg(test)` 下通过 `include!`/`#[path]` 挂载。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 自引用 crate 别名，便于迁移单测按 Go 包路径风格引用本 crate。
extern crate self as astersql_util_sem_v2;

/// SQL 命令名映射与各类 SQL 限制规则实现。
mod sql_rule;
pub use sql_rule::*;
/// SEM 配置结构体与从文件解析/校验逻辑。
mod config;
pub use config::*;
/// SEM 运行时：启用/禁用、全局指针与可见性/权限查询。
mod sem;
pub use sem::*;
/// 受限优化器 hint 判定（可与系统变量联动）。
mod restricted_hint;
pub use restricted_hint::*;
/// 单测辅助：按路径启用 SEM 并在清理时恢复系统变量。
mod testhelper;
pub use testhelper::*;

/// 迁移对齐用 AST/配置回归单测。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// SQL 规则单元测试（解析真实 SQL 后套用规则）。
#[cfg(test)]
mod sql_rule_test {
    use super::*;
    use parser;

    include!("sql_rule_test.rs");
}

/// 配置解析与校验单元测试。
#[cfg(test)]
mod config_test {
    use super::*;

    include!("config_test.rs");
}

/// 受限 hint 单元测试。
#[cfg(test)]
mod restricted_test {
    use super::*;

    include!("restricted_test.rs");
}

/// SEM 运行时方法与 Enable 流程单元测试。
#[cfg(test)]
mod sem_test {
    use super::*;

    include!("sem_test.rs");
}
