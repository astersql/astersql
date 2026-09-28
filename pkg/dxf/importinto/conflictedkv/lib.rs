// Copyright 2026 AsterSQL.

// 冲突 KV（conflictedkv）子 crate 入口。
//
// IMPORT INTO 在写入/排序阶段可能产生主键或唯一索引冲突。
// 本模块聚合收集器（collector）、删除器（deleter）、处理器（handler）
// 与行句柄过滤（row_handle）等能力，对外再导出核心类型与构造函数。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 冲突行收集：将冲突 KV 解码为行并落盘/校验。
mod collector;
pub use collector::*;
/// 冲突 KV 删除：按策略清理已冲突键值。
mod deleter;
pub use deleter::*;
/// 模块级文档说明。
mod doc;
/// 数据/索引 KV 冲突处理管线（快照刷新、缓冲句柄等）。
mod handler;
pub use handler::*;
/// 有界句柄集合与过滤：限制内存中已处理行句柄规模。
mod row_handle;
pub use row_handle::*;

#[cfg(test)]
#[path = "collector_test.rs"]
mod collector_test;
#[cfg(test)]
#[path = "deleter_test.rs"]
mod deleter_test;
#[cfg(test)]
#[path = "handler_test.rs"]
mod handler_test;
#[cfg(test)]
#[path = "row_handle_test.rs"]
mod row_handle_test;
