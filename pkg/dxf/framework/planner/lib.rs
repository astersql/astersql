// Copyright 2026 AsterSQL.

// DXF（Distributed eXecution Framework，分布式执行框架）Planner crate 入口。
//
// 本模块把逻辑计划（LogicalPlan）与物理计划（PhysicalPlan）相关类型重新导出，
// 并串联 proto / storage 依赖，供上层按任务类型生成分布式任务。

#![allow(dead_code)]

/// 逻辑/物理计划类型与 DAG（有向无环图）处理器规格。
pub mod plan;
/// Planner：将逻辑计划序列化为任务 meta 并创建分布式任务。
pub mod planner;

/// 任务/步骤/子任务等协议类型（对齐 Go `proto` 包）。
pub use astersql_dxf_framework_proto as proto;
/// 任务存储与会话上下文边界。
pub use astersql_dxf_framework_storage as storage;
pub use plan::*;
pub use planner::*;

// 物理计划过滤 step、保持 processor 顺序的单元测试。
#[cfg(test)]
#[path = "plan_test.rs"]
mod plan_test;
// Planner 序列化 meta 并转发创建字段的单元测试。
#[cfg(test)]
#[path = "planner_test.rs"]
mod planner_test;
