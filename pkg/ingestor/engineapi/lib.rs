// Copyright 2026 AsterSQL.

// 导入引擎（Engine）与可导入数据（IngestData）的公共 API。
//
// 对应 Go `ingestor/engineapi`：Lightning / IMPORT INTO 将已排序的 KV 写成 SST
// 后直接 ingest 到 TiKV。本 crate 定义引擎、数据源、冲突信息与重复键策略的接口，
// 具体本地/外部引擎实现位于上层 backend。
//
// SST：Sorted String Table，有序键值文件；ingest：跳过常规写路径直接导入到存储引擎。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 引擎接口、键范围与重复键策略。
pub mod engine;
/// 可导入数据、向前迭代器与取消上下文。
pub mod ingest_data;
pub use engine::*;
pub use ingest_data::{Context, EngineError, ForwardIter, IngestData};

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
