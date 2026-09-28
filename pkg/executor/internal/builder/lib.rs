// Copyright 2026 AsterSQL.

// executor 内部 builder crate 入口。
//
// 将物理执行计划（Physical Plan）转换为可下推到存储层的 DAG 请求辅助类型与函数
// 集中在 `builder_utils` 中，此处再行导出。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// DAG 请求构造工具：物理计划到 protobuf 执行器、TiKV/TiFlash 形态组装。
mod builder_utils;
/// 再导出 `builder_utils` 中的公共 API，供执行器构建路径直接使用。
pub use builder_utils::*;

#[cfg(test)]
mod builder_utils_aster_unit_test;
