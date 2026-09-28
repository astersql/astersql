// Copyright 2026 AsterSQL.

//! BR 还原侧 ImportSST 客户端包入口：挂载实现模块并扁平再导出。
//! 与 Go `br/pkg/restore/internal/import_client` 对应——封装向 TiKV
//! 发起 Download/Ingest/Apply 等 import_sstpb RPC 的连接缓存与能力探测。
//! 测试用 `#[path]` 挂载 parity/单元测试，避免与实现文件同目录混编。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

// 显式 path 固定模块文件，便于与 Go 包布局一一对照。
#[path = "import_client.rs"]
pub mod import_client;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "import_client_test.rs"]
mod import_client_test;

// 对外扁平导出 ImporterClient 等符号，调用方不必写 import_client::import_client::。
pub use import_client::*;
