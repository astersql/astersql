// Copyright 2026 AsterSQL.

// 资源管理器协程池（pool）子模块入口。
//
// 对外再导出 `basepool`：提供池关闭/过载/参数非法等哨兵错误，
// 以及可调容的基础池元数据（名称、任务 ID、上次调谐时间戳）。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 基础协程池实现：名称、任务 ID 生成器与调谐时间戳。
pub mod basepool;
pub use basepool::*;

/// 迁移对照单测：校验哨兵错误文案、名称/时间戳语义与并发任务 ID 唯一性。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
