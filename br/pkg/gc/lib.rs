// Copyright 2026 AsterSQL.
//! BR GC 子系统 crate 入口：safepoint 保持与 Manager 抽象的对外导出面。
//!
//! 子模块：`manager`（工厂与 trait）、`manager_global` / `manager_keyspace`（实现）、
//! `safepoint`（服务 safepoint TTL/守护）。测试按 path 挂载，不污染正式导出。
//! `pub use` 汇总备份/恢复调用方常用符号，对齐 Go `br/pkg/gc` 包级可见性。

#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

#[path = "manager.rs"]
pub mod manager;

#[path = "manager_global.rs"]
pub mod manager_global;

#[path = "manager_keyspace.rs"]
pub mod manager_keyspace;

#[path = "safepoint.rs"]
pub mod safepoint;

// 对外重导出 Manager 契约与 PD 客户端子集，供 checkpoint/restore 等依赖。
pub use manager::{
    GCBarrierInfo, GCState, GCStatesClient, KeyspaceID, Manager, NewManager, NullspaceID, PdClient,
};
// safepoint 侧导出默认 TTL、上下文与守护启动入口。
pub use safepoint::{
    BRServiceSafePoint, CheckGCSafePoint, Context, DefaultBRGCSafePointTTL,
    DefaultCheckpointGCSafePointTTL, DefaultStreamPauseSafePointTTL,
    DefaultStreamStartSafePointTTL, MakeSafePointID, StartServiceSafePointKeeper,
};

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "mock_test.rs"]
mod mock_test;

#[cfg(test)]
#[path = "manager_test.rs"]
mod manager_test;

#[cfg(test)]
#[path = "safepoint_test.rs"]
mod safepoint_test;
