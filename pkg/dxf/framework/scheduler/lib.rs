// Copyright 2026 AsterSQL.

// DXF Scheduler 包入口：导出协议、子模块与调度相关公共 API。
//
// 子模块覆盖自动扩缩容、负载均衡、节点管理、槽位、状态机与调度器管理。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 再导出 DXF 协议类型（Task/Subtask 等）。
pub use astersql_dxf_framework_proto as proto;
/// 再导出调度状态相关类型。
pub use astersql_dxf_framework_schstatus as schstatus;

/// 自动扩缩容：按负载调整任务使用的节点规模。
pub mod autoscaler;
/// 子任务负载均衡：在节点间迁移 ExecID。
pub mod balancer;
/// 核心类型、状态常量与 TaskManager/Extension/Scheduler trait。
pub mod interface;
/// 节点管理：存活检测、managed 节点缓存与按 scope 过滤。
pub mod nodes;
/// 单任务 Scheduler 实现。
pub mod scheduler;
/// Scheduler 管理器：创建/销毁各任务的调度器并驱动 tick。
pub mod scheduler_manager;
/// 槽位（Slot）与条带（Stripe）资源预留。
pub mod slots;
/// 任务/子任务状态转换辅助。
pub mod state_transform;

pub use autoscaler::*;
pub use balancer::*;
pub use interface::*;
pub use nodes::*;
pub use scheduler::*;
pub mod storage_adapter;
pub use scheduler_manager::*;
pub use slots::*;
pub use state_transform::*;
pub use storage_adapter::*;

#[cfg(test)]
mod autoscaler_test;
#[cfg(test)]
mod balancer_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod nodes_test;
#[cfg(test)]
mod scheduler_manager_nokit_test;
#[cfg(test)]
mod scheduler_manager_test;
#[cfg(test)]
mod scheduler_nokit_test;
#[cfg(test)]
mod scheduler_test;
#[cfg(test)]
mod slots_test;
#[cfg(test)]
mod test_support;
