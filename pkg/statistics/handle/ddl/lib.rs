// Copyright 2026 AsterSQL.

// 统计信息 DDL 处理 crate 入口。
//
// 聚合 DDL 事件队列（`ddl`）与 schema 变更订阅者（`subscriber`）：
// 在表/分区创建、截断、删除、加列等 DDL（数据定义语言）完成后，
// 同步维护对应的统计元数据与全局统计增量。

#![allow(dead_code)]

/// DDL 事件入队与处理入口（对应 Go `ddl` 包中的 handler）。
pub mod ddl;
/// Schema 变更事件分发与统计后端写入逻辑。
pub mod subscriber;

pub use ddl::*;
pub use subscriber::*;

#[cfg(test)]
#[path = "ddl_test.rs"]
/// DDL 订阅与增量更新路径的单元测试。
mod ddl_test;
