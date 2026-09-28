// Copyright 2026 AsterSQL.

//! Real Apache Parquet dependency boundary for the migrated Go imports.
//!
//! The previous migration draft implemented these packages with a custom byte
//! stream. Re-exporting the actual crate keeps this boundary explicit without
//! weakening the Go program's Parquet, Snappy, dictionary, or schema semantics.
//!
//! 本模块是 Go 依赖迁移到 Rust `parquet` crate 的适配边界，不自行实现文件格式。
//! 直接转出官方实现可保留 Parquet 编解码、Snappy 压缩、字典编码与 schema 语义，
//! 同时让生成器其余代码只依赖这一组明确的模块入口。

// 保持这些模块按原层级公开，调用方可沿用迁移后的依赖路径而无需复制实现。
pub use ::parquet::basic;
pub use ::parquet::column;
pub use ::parquet::data_type;
pub use ::parquet::file;
pub use ::parquet::schema;
