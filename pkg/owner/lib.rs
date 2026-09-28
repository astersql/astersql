// Copyright 2026 AsterSQL.
// Owner（所有者）选举 crate 入口。
//
// 对应 Go 的 `pkg/owner`：基于 etcd 租约与竞选（campaign），在多 TiDB 实例间
// 选出唯一 Owner，驱动 DDL、统计信息等需要单点协调的后台任务。本文件组织
// manager / mock 子模块，并在测试配置下挂载失败路径与集成测试。

#![allow(non_snake_case)]

extern crate self as astersql_owner;
/// etcd 实现的 OwnerManager 与分布式锁。
pub mod manager;
/// 本地 Mock Owner，不依赖真实 etcd。
pub mod mock;
/// Mock 用的全局 Owner 状态表。
pub mod mock_owner_state;
pub use manager::*;
pub use mock::*;
pub use mock_owner_state::*;

/// Aster 迁移单测：编解码、Mock 竞争与 ListenersWrapper。
#[cfg(test)]
mod manager_1_aster_unit_test {
    use crate as astersql_owner;
    include!("manager_1_aster_unit_test.rs");
}

/// etcd 传输失败时新建 session 的回归测试。
#[cfg(test)]
mod fail_test {
    use crate as owner;
    include!("fail_test.rs");
}

/// 嵌入式 etcd 上的 Owner 竞选、强制接管、Watch、分布式锁等集成测试。
#[cfg(test)]
mod manager_test {
    use crate as owner;
    include!("manager_test.rs");
}

/// Mock Manager 与 Go 进程级共享状态语义的回归测试。
#[cfg(test)]
#[path = "mock_test.rs"]
mod mock_test;
