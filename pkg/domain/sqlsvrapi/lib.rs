// Copyright 2026 AsterSQL.

// sqlsvrapi：SQL Server 运行时抽象的 crate 入口。
//
// 重新导出 KV Storage、元数据模型、DDL owner、session pool 等依赖别名，
// 并暴露 `server` 模块中的 `Runtime` / `KSRuntimeHandle` / `Server` 接口。
// Keyspace 表示多租户命名空间；本包供跨 keyspace DDL 等场景获取目标运行时。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]
extern crate self as astersql_domain_sqlsvrapi;
/// KV Storage 依赖再导出。
pub mod kv {
    pub use kv_dependency::Storage;
}
/// 元数据模型（含 AlterTableModeTarget / TableMode）。
pub mod meta {
    pub mod model {
        pub use model_dependency::group_3::*;
    }
}
/// DDL owner Manager 依赖再导出。
pub mod owner {
    pub use owner_dependency::Manager;
}
/// session pool（DestroyableSessionPool）依赖再导出。
pub mod util {
    pub use util_dependency::session_pool::DestroyableSessionPool;
}
/// 测试用 KV 支持包别名。
pub use kv_dependency as kv_test_support;
/// 测试用 owner 支持包别名。
pub use owner_dependency as owner_test_support;
/// 测试用 util 支持包别名。
pub use util_dependency as util_test_support;
/// Runtime / Server / KSRuntimeHandle 接口定义。
pub mod server;

/// 迁移期 Aster 单元测试：手写 Recording* 桩验证接口契约。
#[cfg(test)]
mod migration_aster_unit_test;
