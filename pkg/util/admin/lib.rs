// Copyright 2026 AsterSQL.

// `util/admin` crate 入口：导出管理校验 API，并挂接集成/单元测试模块。
//
// 对应 Go `util/admin` 包，提供表与索引一致性检查、行迭代与记录键编解码等能力。

#![allow(non_snake_case, non_upper_case_globals)]

mod admin;
pub use admin::*;

#[cfg(test)]
#[path = "admin_integration_test.rs"]
mod admin_integration_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
