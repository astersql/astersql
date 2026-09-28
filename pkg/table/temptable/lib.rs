// Copyright 2026 AsterSQL.

// `temptable` crate：会话级临时表（Temporary Table）支持（对应 Go `pkg/table/temptable`）。
//
// 提供本地临时表的 DDL（创建/删除/截断）、会话 InfoSchema 扩展，
// 以及 Snapshot 拦截器：把临时表键读写路由到会话内存缓冲（MemBuffer），
// 与持久化表的 TiKV Snapshot（某时间点一致性视图）合并。

#![allow(dead_code)]

/// 本地临时表 DDL：创建、删除与截断。
pub mod ddl;
/// 临时表元数据与会话 InfoSchema（信息模式，表/库目录）扩展。
pub mod infoschema;
/// Snapshot 拦截与会话 MemBuffer / UnionIter 合并读取。
pub mod interceptor;

/// 再导出 DDL、InfoSchema、拦截器公共 API。
pub use ddl::*;
pub use infoschema::*;
pub use interceptor::*;

// 测试模块：主夹具、DDL 与拦截器用例。
#[cfg(test)]
#[path = "ddl_test.rs"]
mod ddl_test;
#[cfg(test)]
#[path = "infoschema_test.rs"]
mod infoschema_test;
#[cfg(test)]
#[path = "interceptor_test.rs"]
mod interceptor_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
