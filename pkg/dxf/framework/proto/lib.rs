// Copyright 2026 AsterSQL.
// DXF 协议类型 crate 入口。
//
// 汇总任务（Task）、子任务（Subtask）、步骤（Step）、节点资源（Node）、
// 任务修改（Modify）与任务类型（Type）等协议定义，并对齐 Go `proto` 包
// 的命名与常量语义，供调度器、执行器与存储层共享。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 运行时修改任务参数（并发/节点数等）相关类型。
pub mod modify;
/// 受管节点与节点资源（CPU/内存/磁盘）快照。
pub mod node;
/// 任务 step 常量及 step↔字符串转换。
pub mod step;
/// 子任务与可分配资源（Allocatable / StepResource）。
pub mod subtask;
/// 任务主体、状态机与 ExtraParams。
pub mod task;
/// 任务类型字符串常量（Backfill、ImportInto 等）。
pub mod r#type;
pub use modify::*;
pub use node::*;
pub use step::*;
pub use subtask::*;
pub use task::*;
pub use r#type::*;

// 与 Go 对齐的迁移回归：step/type/state/node/subtask/modification。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

// step 转换与合法性校验单测。
#[cfg(test)]
#[path = "step_test.rs"]
mod step_test;

// 子任务与资源分配单测。
#[cfg(test)]
#[path = "subtask_test.rs"]
mod subtask_test;

// 任务状态、ExtraParams 与排序单测。
#[cfg(test)]
#[path = "task_test.rs"]
mod task_test;

// 任务类型编解码单测。
#[cfg(test)]
#[path = "type_test.rs"]
mod type_test;
