// Copyright 2026 AsterSQL.

// Lightning KV 编码后端子 crate 入口。
//
// 聚合分配器、基座编码器、编码上下文、KV↔SQL 转换与轻量 Session，
// 供数据导入路径将 SQL 行值编码为 TiKV 键值对（或反向解码）。

#![allow(non_snake_case, non_upper_case_globals, non_camel_case_types)]

mod allocator;
mod base;
mod canonical;
mod context;
mod kv2sql;
mod session;
mod sql2kv;

pub use allocator::*;
pub use base::*;
pub use canonical::*;
pub use context::*;
pub use kv2sql::*;
pub use session::*;
pub use sql2kv::*;

#[cfg(test)]
#[path = "allocator_test.rs"]
mod allocator_test;
#[cfg(test)]
#[path = "base_test.rs"]
mod base_test;
#[cfg(test)]
#[path = "canonical_test.rs"]
mod canonical_test;
#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;
#[cfg(test)]
#[path = "kv2sql_test.rs"]
mod kv2sql_test;
#[cfg(test)]
#[path = "session_internal_test.rs"]
mod session_internal_test;
#[cfg(test)]
#[path = "session_test.rs"]
mod session_test;
#[cfg(test)]
#[path = "sql2kv_test.rs"]
mod sql2kv_test;
