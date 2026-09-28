// Copyright 2026 AsterSQL.

//! ingestrec 包入口：复原日志备份中 ingest 模式索引与外键约束所需元数据。
//! 对应 Go `br/pkg/restore/ingestrec`；真实逻辑在 `model_stub`/`foreign_key`/
//! `ingest_recorder`，本文件只做模块挂载与扁平再导出。
//! 测试经 `#[path]` 挂到独立文件，避免与实现混编（对齐仓库 Rust 约定）。
//! `model_stub` 提供 meta/infoschema 本地替身，使本 crate 脱离 grpcio 重建路径。
//! `foreign_key` 记录修复索引前需先删除的 FK；`ingest_recorder` 过滤 ingest DDL job。
//! 调用方应 `use` 本 crate 再导出符号，勿直接依赖内部 `#[path]` 模块路径。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

#[path = "model_stub.rs"]
pub mod model_stub;

#[path = "foreign_key.rs"]
pub mod foreign_key;

#[path = "ingest_recorder.rs"]
pub mod ingest_recorder;

#[cfg(test)]
#[path = "export_test.rs"]
mod export_test;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "model_stub_test.rs"]
mod model_stub_test;

#[cfg(test)]
#[path = "foreign_key_test.rs"]
mod foreign_key_test;

#[cfg(test)]
#[path = "ingest_recorder_test.rs"]
mod ingest_recorder_test;

// 扁平再导出：与 Go 同包可见性一致，restore 上层直接使用录制器/FK 类型。
pub use foreign_key::*;
pub use ingest_recorder::*;
pub use model_stub::*;
