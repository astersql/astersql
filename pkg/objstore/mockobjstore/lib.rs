// Copyright 2026 AsterSQL.

// 对象存储（Object Store）的 Mock 实现 crate 入口。
//
// 对外重导出 `objectio` 的 `Context`/`Reader`/`Writer`，以及 `storeapi`；
// 实际 Mock 逻辑在 `mockobjstore`（`objstore_mock.rs`）中，供单元测试按预期
// 顺序注册与校验 Storage 调用。

#![allow(non_snake_case, non_upper_case_globals)]

/// 重导出 objectio 取消上下文与读写流接口。
pub use objectio::{Context, Reader, Writer};

/// storeapi 兼容层：重导出对象存储 Storage trait 及相关类型。
pub mod storeapi {
    pub use astersql_objstore_storeapi::*;
}

#[path = "objstore_mock.rs"]
/// 严格 Mock 的 Storage 实现（对齐 GoMock 期望调用语义）。
pub mod mockobjstore;
pub use mockobjstore::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移用单元测试：校验 Mock 转发顺序与期望结果。
mod migration_aster_unit_test;
