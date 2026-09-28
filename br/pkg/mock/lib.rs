// Copyright 2026 AsterSQL.
//
// 本文件是 `br/pkg/mock` 的 Rust 入口与模块装配层。
// 对应 Go `package mock`：把 gomock 生成文件、集群桩与任务注册拼成可被上层测试引用的 crate。
// 这里不承载真实导入/备份逻辑，只决定子模块加载顺序与平铺导出集合。
// 生产代码路径不应依赖这些 mock；它们服务于隔离测试与 Go 行为对齐验证。
// 先挂 stubs（类型/Controller），再挂 backend/common/encode/importer 等生成物。
// mock_cluster / task_register 提供更接近集成场景的组合桩。
// 最后的 `pub use` 模拟 Go 包级符号可见性，避免调用方感知文件切分。
// 测试模块仅在 `#[cfg(test)]` 下挂接，避免污染正常编译依赖图。
// parity_test 与 mock_cluster_test 分别覆盖符号对齐与集群桩行为。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

#[path = "stubs.rs"]
pub mod stubs;

#[path = "backend.rs"]
pub mod backend;

#[path = "common.rs"]
pub mod common;

#[path = "encode.rs"]
pub mod encode;

#[path = "importer.rs"]
pub mod importer;

#[path = "mock_cluster.rs"]
pub mod mock_cluster;

#[path = "task_register.rs"]
pub mod task_register;

// 平铺导出各 mock 与 stubs 公共类型，对齐 Go 包导入体验。
pub use backend::*;
pub use common::*;
pub use encode::*;
pub use importer::*;
pub use mock_cluster::*;
pub use stubs::{
    Call, CallOption, CheckCtx, ChunkFlushStatus, CleanupEngineRequest, CleanupEngineResponse,
    CloseEngineRequest, CloseEngineResponse, CompactClusterRequest, CompactClusterResponse,
    Context, Controller, DBInfo, Datum, EncodingConfig, EngineConfig, EngineWriter, Error,
    GetMetricsRequest, GetMetricsResponse, GetVersionRequest, GetVersionResponse, HttpServer,
    ImportEngineRequest, ImportEngineResponse, KVChecksum, LocalWriterConfig, Metadata,
    MysqlConfig, OpenEngineRequest, OpenEngineResponse, PDClient, PDHTTPClient, Result, RowHandle,
    RowsHandle, Server, SimpleChunkFlushStatus, Storage, SwitchModeRequest, SwitchModeResponse,
    TableInfo, TiKVCluster, UUID, WriteEngineClient, WriteEngineRequest, WriteEngineResponse,
    WriteEngineV3Request,
};
pub use task_register::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "mock_cluster_test.rs"]
mod mock_cluster_test;

#[cfg(test)]
#[path = "importer_test.rs"]
mod importer_test;
