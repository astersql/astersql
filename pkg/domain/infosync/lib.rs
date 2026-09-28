// Copyright 2026 AsterSQL.

// infosync（信息同步）crate 入口。
//
// 负责 TiDB 节点与 PD / etcd / TiFlash 等组件之间的集群元信息同步，
// 包括标签规则、放置策略（placement）、调度配置、资源组、Region
//（TiKV 数据分片）复制状态，以及服务器拓扑信息。本文件重新导出子模块。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]

/// DDL Label（标签）规则类型，对齐 Go 侧 `label` 包路径。
pub use ddl_label as label;
/// DDL Placement（放置策略）类型，对齐 Go 侧 `placement` 包路径。
pub use ddl_placement as placement;
/// 元数据模型（表/分区等），对齐 Go 侧 `model` 包路径。
pub use meta_model as model;

/// 错误类型与 Result 别名。
mod error;
pub use error::*;
/// 公共数据结构与 PD HTTP 客户端 trait。
mod types;
pub use types::*;
/// Region 标签规则管理器（PD / mock）。
mod label_manager;
pub use label_manager::*;
/// 放置策略 Bundle 管理器（PD / mock）。
mod placement_manager;
pub use placement_manager::*;
/// PD 调度配置读写管理器。
mod schedule_manager;
pub use schedule_manager::*;
/// 资源管理客户端（资源组 CRUD）。
mod resource_manager_client;
pub use resource_manager_client::*;
/// TiFlash 副本与放置规则管理。
mod tiflash_manager;
pub use tiflash_manager::*;
/// 测试用全局 ServerInfo 管理器与 ServerInfo 结构。
mod mock_info;
pub use mock_info::*;
/// InfoSyncer 核心：全局单例、拓扑查询与对外 API。
mod info;
pub use info::*;
/// Region 复制状态与调度器配置相关 API。
mod region;
pub use region::*;

#[cfg(test)]
#[path = "info_test.rs"]
mod info_test;
#[cfg(test)]
#[path = "label_manager_test.rs"]
mod label_manager_test;
#[cfg(test)]
#[path = "region_test.rs"]
mod region_test;
#[cfg(test)]
#[path = "resource_manager_client_test.rs"]
mod resource_manager_client_test;
#[cfg(test)]
#[path = "tiflash_manager_test.rs"]
mod tiflash_manager_test;
#[cfg(test)]
#[path = "types_test.rs"]
mod types_test;
