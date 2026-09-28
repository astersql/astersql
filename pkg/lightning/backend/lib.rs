// Copyright 2026 AsterSQL.

// Lightning 后端抽象层入口。
//
// 再导出 `backend` 模块中的 Backend trait、引擎写入器与配置类型，
// 供 KV（物理导入）与 TiDB（逻辑导入，经 SQL 写入）等具体后端实现。

#![allow(non_snake_case, non_upper_case_globals, non_camel_case_types)]

mod backend;
pub use backend::*;

#[cfg(test)]
#[path = "backend_test.rs"]
mod backend_test;
