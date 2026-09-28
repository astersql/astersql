// Copyright 2026 AsterSQL.

// `poolmanager` crate 入口：拼装任务元数据、迭代器与调度器。
//
// 任务管理器（TaskManager）按分片登记并发任务，支持 Overclock/Downclock
// 在资源管理器调容时增减运行中的 worker。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

include!("task_manager.rs");
include!("task_manager_iterator.rs");
include!("task_manager_scheduler.rs");

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
