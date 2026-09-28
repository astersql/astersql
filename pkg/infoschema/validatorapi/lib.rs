// Copyright 2026 AsterSQL.

// InfoSchema 校验器（schemaValidator）的对外 API crate。
//
// 对应 Go 的 `validator` 接口层：事务在使用某个 schema 版本前，需确认该版本在
// 租约（lease）窗口内仍然有效，避免读到已过期的元数据。本 crate 只暴露接口与
// 结果枚举，具体实现由上层注入。
//
// schema 版本：DDL 变更递增的元数据版本号；租约：PD/TiDB 授予的 schema 有效期窗口。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 校验器接口与结果枚举定义。
pub mod interface;
pub use interface::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
mod interface_test;
