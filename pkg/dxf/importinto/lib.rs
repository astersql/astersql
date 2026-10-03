// Copyright 2026 AsterSQL.

// IMPORT INTO 分布式执行包入口。
//
// 组装协议元数据、指标、冲突处理、编码排序算子、子任务/任务执行器、
// 物理计划、作业管理、清理与调度器等子模块，并在测试配置下挂载对应单测。

#![allow(dead_code)]

/// 任务/步骤元数据与序列化协议。
pub mod proto;
pub use proto::*;
/// 按分布式任务注册/注销 Prometheus 导入指标。
pub mod metrics;
pub use metrics::*;

/// 收集冲突 KV 步骤实现。
pub mod collect_conflicts;
pub use collect_conflicts::*;
/// 冲突解决（删除冲突行等）步骤实现。
pub mod conflict_resolution;
pub use conflict_resolution::*;
/// 编码并外部排序的算子管线。
pub mod encode_and_sort_operator;
pub use encode_and_sort_operator::*;
/// 子任务执行器（单步 worker）。
pub mod subtask_executor;
pub use subtask_executor::*;
/// 顶层任务执行器（编排各步骤）。
pub mod task_executor;
pub use task_executor::*;
/// 逻辑计划到物理计划/子任务 meta 的规划器。
pub mod planner;
pub use planner::*;
/// IMPORT INTO 作业生命周期管理。
pub mod job;
pub use job::*;
/// 导入结束后的表模式恢复与资源清理。
pub mod clean_up;
pub use clean_up::*;
/// 分布式任务调度器适配。
pub mod scheduler;
pub use scheduler::*;

#[cfg(test)]
#[path = "clean_up_test.rs"]
mod clean_up_test;
#[cfg(test)]
#[path = "collect_conflicts_test.rs"]
mod collect_conflicts_test;
#[cfg(test)]
#[path = "conflict_resolution_test.rs"]
mod conflict_resolution_test;
#[cfg(test)]
#[path = "encode_and_sort_operator_test.rs"]
mod encode_and_sort_operator_test;
#[cfg(test)]
#[path = "job_test.rs"]
mod job_test;
#[cfg(test)]
#[path = "job_testkit_test.rs"]
mod job_testkit_test;
#[cfg(test)]
#[path = "metrics_test.rs"]
mod metrics_test;
#[cfg(test)]
#[path = "planner_test.rs"]
mod planner_test;
#[cfg(test)]
#[path = "proto_test.rs"]
mod proto_test;
#[cfg(test)]
#[path = "scheduler_test.rs"]
mod scheduler_test;
#[cfg(test)]
#[path = "scheduler_testkit_test.rs"]
mod scheduler_testkit_test;
#[cfg(test)]
#[path = "subtask_executor_test.rs"]
mod subtask_executor_test;
#[cfg(test)]
#[path = "task_executor_test.rs"]
mod task_executor_test;
#[cfg(test)]
#[path = "task_executor_testkit_test.rs"]
mod task_executor_testkit_test;

pub mod write_ingest_backend;
