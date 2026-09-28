// Copyright 2026 AsterSQL.

// 事务事件（trxevents）crate 入口。
//
// 导出 `trx_events` 模块中的 TransactionEvent / CopMeetLock 等类型；
// 测试编译时挂载 migration 补充单测。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 事务事件类型与包装函数。
pub mod trx_events;
pub use trx_events::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
