// Copyright 2026 AsterSQL.

// util 包专用 Mock 工具 crate 入口。
//
// 导出测试用 KV Client、会话 Context、迭代器、指标计数器与 Store 等桩实现；
// 测试配置下挂载 `TestMain`、迭代器测试与迁移回归用例。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

extern crate self as mock_crate;
/// 依赖的 KV 相关类型再导出。
pub use kv_crate as kv;
/// 会话上下文相关类型再导出。
pub use sessionctx_crate as sessionctx;
/// SLI（服务水平指标）相关类型再导出。
pub use sli_crate as sli;
/// 系统变量定义再导出。
pub use vardef_crate as vardef;
/// 会话变量实现再导出。
pub use variable_crate as variable;

/// Mock KV Client。
mod client;
pub use client::*;
/// Mock 会话 Context。
mod context;
pub use context::*;
/// 测试专用 Context 工厂。
mod fortest;
pub use fortest::*;
/// Mock KV 迭代器。
mod iter;
pub use iter::*;
/// 测试用指标计数器。
mod metrics;
pub use metrics::*;
/// Mock KV Store。
mod store;
pub use store::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充回归测试。
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod mock_main_test;

#[cfg(test)]
#[path = "iter_test.rs"]
/// `SliceIter` 单元测试。
mod iter_test;

#[cfg(test)]
#[path = "mock_test.rs"]
/// Mock 包其它单元测试。
mod mock_test;

#[cfg(test)]
#[path = "go_merge_30_test.rs"]
mod go_merge_30_test;
