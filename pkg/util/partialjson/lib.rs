// Copyright 2026 AsterSQL.

// 部分 JSON（partial JSON）解析 crate 入口。
//
// 对应 Go `pkg/util/partialjson`。只抽取顶层对象中指定键的值，跳过未请求的嵌套结构，
// 用于大 JSON 中按需读取少量字段而不必完整反序列化。核心实现在 `extract` 子模块。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 顶层 JSON 成员按需抽取与流式 token 迭代。
pub mod extract;
pub use extract::*;

#[cfg(test)]
#[path = "extract_test.rs"]
mod extract_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
