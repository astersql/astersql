// Copyright 2026 AsterSQL.

// DXF example crate 根模块。
//
// 汇出 proto（任务/子任务 meta）、scheduler（示例调度器）与
// task_executor（示例执行器），并挂载 `app_test` 端到端测试。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_variables
)]
/// 示例模块说明文档。
mod doc;
/// 任务与子任务 meta 的简易编解码。
mod proto;
/// 示例两步调度器与清理钩子。
mod scheduler;
/// 示例任务执行器与 step 执行器。
mod task_executor;
/// 对外再导出 proto 类型与编解码方法。
pub use proto::*;
/// 对外再导出调度器、step 常量与清理实现。
pub use scheduler::*;
/// 对外再导出任务/步骤执行器构造入口。
pub use task_executor::*;

#[cfg(test)]
#[path = "app_test.rs"]
/// 端到端应用测试（仅 test 配置）。
mod app_test;

#[cfg(test)]
#[path = "proto_test.rs"]
/// Meta JSON 与 Go encoding/json 的一致性测试。
mod proto_test;

#[cfg(test)]
#[path = "task_executor_test.rs"]
/// Task executor 与 Go 实现的行为一致性测试。
mod task_executor_test;
