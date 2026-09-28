// Copyright 2026 AsterSQL.

// InfoSchema 校验器（isvalidator）crate 入口。
//
// 对应 Go 的 `pkg/infoschema/isvalidator`：在 schema 版本切换后，
// 校验进行中的事务是否仍可安全使用旧 Information Schema（元数据内存视图）。
// 若相关表已变更且租约过期，则应使依赖旧版本的事务失败，避免读写不一致。
//
// 本文件重导出依赖与 `validator` 实现，并挂载单元测试。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 日志工具（与 Go logutil 对齐）。
pub use logutil;
/// SchemaValidator 对外 API 定义。
pub use validatorapi;
/// 会话 / 系统变量相关定义（测试会改全局变量，故需锁）。
pub use vardef;

/// Schema 校验器实现（租约、相关表变更跟踪等）。
#[path = "validator.rs"]
pub mod validator;
pub use validator::*;

/// 测试间互斥：部分用例会修改全局 vardef，避免并行干扰。
#[cfg(test)]
pub(crate) static VARDEF_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
mod validator_test {
    include!("validator_test.rs");
}
