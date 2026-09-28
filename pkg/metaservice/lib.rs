// Copyright 2026 AsterSQL.

// metaservice crate 根模块：元数据服务客户端与管理器入口。
//
// 元数据服务对接 PD（Placement Driver，集群调度中心）成员列表与 etcd 客户端持有关系；
// `etcd` 提供 PD 地址解析与重试，`metamanager` 提供更高层的元数据管理封装。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 基于 etcd / PD 的 MetaService 客户端实现。
pub mod etcd;
/// 元数据管理器相关类型与逻辑。
pub mod metamanager;
pub use etcd::*;
pub use metamanager::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
