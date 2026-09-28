// Copyright 2026 AsterSQL.

// CPU 剖析测试辅助子 crate 入口。
//
// 导出 `util` 中的取消令牌、模拟 CPU 负载等工具，并挂接迁移期单元测试。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 模拟 CPU 负载与标签构造工具。
pub mod util;
pub use util::*;

/// 校验 mock 负载标签分组与 Go 侧一致的迁移测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
