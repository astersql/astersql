// Copyright 2026 AsterSQL.

// Placement（放置策略）crate 入口。
//
// 本 crate 将 SQL 层的 Placement Policy（放置策略）与 Placement Settings
// （放置选项）转换为 PD（Placement Driver，集群调度中枢）可识别的
// Bundle（规则组）与 Rule（放置规则），用于把 Region（数据分片）
// 调度到满足标签约束的 Store 上。
//
// 模块划分：
// - [`errors`]：解析/构造错误；
// - [`common`]：组 ID、Rule Index、标签键值等常量；
// - [`constraint`] / [`constraints`]：单条与成组标签约束；
// - [`rule`] / [`bundle`]：规则与规则组的构造、合并与序列化。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 再导出 PD placement 相关类型（Rule、LabelConstraint、PeerRole 等）。
pub mod pd {
    pub use pdtypes::placement::*;
    // Go imports pd/client/http here, whose Rule includes is_witness.
    pub use pdtypes::placement::HttpRule as Rule;
}
/// 再导出元模型中的放置策略与表/分区信息类型。
pub mod model {
    pub use meta_model::group_3::{PlacementSettings, PolicyInfo};
    pub use meta_model::group_4::{PartitionDefinition, TableInfo};
}
/// 再导出键编码工具（EncodeBytes），用于生成 Rule 的起止键十六进制。
pub mod codec {
    pub use tablecodec_dependency::codec::EncodeBytes;
}
/// 再导出表前缀生成函数，用于按表/分区 ID 划定 key 范围。
pub mod tablecodec {
    pub use tablecodec_dependency::GenTablePrefix;
}
mod errors;
pub use errors::*;
mod common;
pub use common::*;
mod constraint;
pub use constraint::*;
mod constraints;
pub use constraints::*;
mod rule;
pub use rule::*;
mod bundle;
pub use bundle::*;

#[cfg(test)]
#[path = "bundle_1_aster_unit_test.rs"]
mod bundle_1_aster_unit_test;
#[cfg(test)]
mod bundle_test;
#[cfg(test)]
mod common_test;
#[cfg(test)]
mod constraint_test;
#[cfg(test)]
mod constraints_test;
#[cfg(test)]
mod meta_bundle_test;
#[cfg(test)]
mod rule_test;
