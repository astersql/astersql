// Copyright 2026 AsterSQL.

// `util/resourcegrouptag` crate 入口：资源组标签解码与请求首键提取。
//
// 对应 Go `pkg/util/resourcegrouptag`。资源组（Resource Group）用于按租户/业务
// 隔离 CPU、IO 等资源；标签（tag）随 TiKV RPC 下发，本 crate 解码 tipb 标签、
// 按键类型打 Label，并从各类请求中取出用于归因的首个键。

#![allow(non_snake_case)]

/// 由 build.rs 从官方 kvproto 生成的 protobuf 绑定。
#[allow(clippy::all, static_mut_refs)]
pub mod kvproto {
    include!(concat!(env!("OUT_DIR"), "/kvproto/mod.rs"));
}

/// 由 build.rs 从 tipb resourcetag.proto 生成的绑定。
#[allow(static_mut_refs)]
pub mod tipb {
    include!(concat!(env!("OUT_DIR"), "/tipb/mod.rs"));
}

/// 行/索引键种类判定，复用 tablecodec 的 rowindexcodec。
pub mod rowindexcodec {
    pub use astersql_tablecodec_rowindexcodec::*;
}

/// 资源组标签解码与 GetFirstKeyFromRequest 等逻辑。
pub mod resource_group_tag;
/// 再导出 resource_group_tag 公开 API。
pub use resource_group_tag::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移回归：解码、Label 分类与各请求分支首键。
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "resource_group_tag_test.rs"]
/// 对应 Go resource_group_tag_test 的单元测试。
mod resource_group_tag_test;
