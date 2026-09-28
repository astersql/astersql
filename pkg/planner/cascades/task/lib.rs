// Copyright 2026 AsterSQL.

// Cascades 优化任务（task）包入口。
//
// Cascades 将搜索过程拆成可压栈的任务：应用规则、优化 Group /
// GroupExpression、以及任务调度器。本包聚合这些子模块并再导出
// `cascades_base` 中的 `Scheduler` / `Task` 与字符串缓冲写入接口。

#![allow(non_snake_case)]

/// 任务共享上下文与 BaseTask。
mod base;
/// 任务栈（LIFO）与错误类型。
mod task;
/// 对组表达式应用变换规则的任务。
mod task_apply_rule;
/// 优化整个 Group 的任务。
mod task_opt_group;
/// 优化单个 GroupExpression 的任务。
mod task_opt_group_expression;
/// 任务调度器实现。
mod task_scheduler;

pub use base::*;
pub use task::*;
pub use task_apply_rule::*;
pub use task_opt_group::*;
pub use task_opt_group_expression::*;
pub use task_scheduler::*;

pub use cascades_memo::{Group, GroupExpression, GroupExpressionRef, GroupRef};
pub use cascades_rule::{BoundPlan, Rule};
pub use logicalop::LogicalPlanRef;

pub use cascades_base::util::StrBufferWriter;
pub use cascades_base::{Scheduler, Task};

#[cfg(test)]
#[path = "task_test.rs"]
mod task_test;

#[cfg(test)]
#[path = "task_scheduler_test.rs"]
mod task_scheduler_test;
