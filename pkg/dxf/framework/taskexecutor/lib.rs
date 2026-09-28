// Copyright 2026 AsterSQL.

// DXF（Distributed eXecution Framework，分布式执行框架）任务执行器包入口。
//
// 本 crate 对应节点侧 taskexecutor：在 follower/执行节点上轮询可执行任务、
// 按 CPU slot 分配资源、通过工厂创建 `TaskExecutor`，并驱动 subtask（子任务）运行。
// Owner 节点负责调度，本包关注执行与本地资源管理。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_variables
)]

/// 任务执行器接口与类型定义。
mod interface;
/// 节点侧任务执行管理器：轮询可执行任务、分配 slot、拉起 executor。
mod manager;
/// 按任务类型注册/查找 `TaskExecutor` 工厂。
mod register;
/// CPU slot（执行槽位）分配与抢占。
mod slot;
/// 单任务执行器基类：拉取并运行 subtask（子任务）。
mod task_executor;

// 对外再导出各子模块的公开 API。
pub use interface::*;
pub use manager::*;
pub use register::*;
pub use slot::*;
pub use task_executor::*;

#[cfg(test)]
/// 测试入口辅助：缩短轮询间隔。
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
/// Manager 生命周期与抢占测试。
#[path = "manager_test.rs"]
mod manager_test;
#[cfg(test)]
/// 工厂注册表相关测试。
#[path = "register_test.rs"]
mod register_test;
#[cfg(test)]
/// slot 分配与抢占测试。
#[path = "slot_test.rs"]
mod slot_test;
#[cfg(test)]
/// BaseTaskExecutor 行为测试。
#[path = "task_executor_test.rs"]
mod task_executor_test;
#[cfg(test)]
/// 基于 testkit 的执行器集成测试。
#[path = "task_executor_testkit_test.rs"]
mod task_executor_testkit_test;
