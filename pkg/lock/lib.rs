// Copyright 2026 AsterSQL.

// 表锁检查（table lock checker）crate 入口。
//
// 在执行 DDL/DML 前，对照会话持有的表锁与 InfoSchema（信息系统）快照中的
// 表锁元数据，判断当前权限是否允许操作；再导出 `Checker` 及相关错误。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 表锁检查器与权限匹配逻辑实现。
mod lock;
pub use lock::*;

#[cfg(test)]
#[path = "lock_aster_unit_test.rs"]
mod lock_aster_unit_test;
