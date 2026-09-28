// Copyright 2026 AsterSQL.

// MPP 协调器管理器（mppcoordmanager）子模块入口。
//
// MPP（Massively Parallel Processing，大规模并行处理）在 TiFlash 等节点
// 上并行执行查询片段；本模块登记、回收各 gather 对应的协调器实例。

#![allow(dead_code)]

/// MPP 协调器注册表与生命周期管理实现。
pub mod mpp_coordinator_manager;

pub use mpp_coordinator_manager::*;

#[cfg(test)]
#[path = "mpp_coordinator_manager_test.rs"]
mod mpp_coordinator_manager_test;
