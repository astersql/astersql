// Copyright 2026 AsterSQL.

// `mdldef` crate 入口：导出 JobMDL 定义，避免与 issyncer 主包循环依赖。
//
// 对应 Go 将 MDL（Metadata Lock，元数据锁）相关结构单独放在子目录的做法：
// DDL 作业要求实例加载到指定 schema 版本后，访问相关表的会话才能继续。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// JobMDL 等 MDL 作业状态定义。
pub mod mdl;
pub use mdl::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
