// Copyright 2026 AsterSQL.

// 基于表存储的 Timer 持久化实现。
//
// 通过 SQL 访问元数据表，并用 etcd 通知器在集群间广播定时器变更事件。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// etcd 通知器：跨节点广播定时器变更。
mod notifier;
/// 定时器表 SQL 拼装与解析。
mod sql;
/// 基于表存储的 `TimerStore` 实现。
mod store;

/// 对外再导出通知、SQL 与存储实现。
pub use notifier::*;
pub use sql::*;
pub use store::*;

#[cfg(test)]
#[path = "sql_test.rs"]
/// SQL 相关单元测试。
mod sql_test;

#[cfg(test)]
#[path = "store_test.rs"]
/// 表存储 CRUD 与会话状态单元测试。
mod store_test;
