// Copyright 2026 AsterSQL.

// Lightning 配置子 crate 入口。
//
// 聚合字节大小解析、完整任务配置、配置列表、默认常量、全局 CLI 配置，
// 以及 TOML 编解码辅助，并再导出供外部统一使用。

#![allow(dead_code)]

/// 字节容量（ByteSize）解析与显示。
pub mod bytesize;
/// 完整 Lightning 任务配置结构与加载逻辑。
pub mod config;
/// 多配置项列表相关类型。
pub mod configlist;
/// 默认常量与 gRPC keepalive 参数（模块名 `const` 为关键字，故用 `r#const`）。
pub mod r#const;
/// 全局 CLI / 轻量配置加载（对应 Go `GlobalConfig`）。
pub mod global;
/// TOML 加载与部分字段编码辅助。
pub mod toml_codec;

pub use bytesize::*;
pub use config::*;
pub use configlist::*;
pub use r#const::*;
pub use global::*;

#[cfg(test)]
#[path = "bytesize_test.rs"]
mod bytesize_test;

#[cfg(test)]
#[path = "configlist_test.rs"]
mod configlist_test;

#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;

#[cfg(test)]
#[path = "toml_codec_test.rs"]
mod toml_codec_test;
