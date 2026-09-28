// Copyright 2026 AsterSQL.

// DDL 内部会话（session）子模块入口。
//
// 导出会话上下文与会话池相关类型，供 DDL 作业在后台执行 SQL
// （如元数据写入、系统表维护）时复用内部会话，而不走普通用户连接路径。

#![allow(dead_code)]

/// 会话上下文与执行辅助实现。
pub mod session;
/// 内部会话资源池（借出/归还/销毁）。
pub mod session_pool;

pub use session::*;
pub use session_pool::*;

#[cfg(test)]
#[path = "session_pool_test.rs"]
mod session_pool_test;
