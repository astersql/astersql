// Copyright 2026 AsterSQL.

//! BR 的 TiKV Glue 包入口：把 `glue` 实现模块挂到 crate 根并再导出。
//! 与 Go `br/pkg/gluetikv` 对应——提供不依赖 TiDB SQL 层的 KV 侧 glue，
//! 供 backup/restore 任务在纯 TiKV 场景复用。测试用 `#[path]` 挂载
//! parity/glue 单测，避免与实现文件同目录混编。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 显式 path 固定模块文件，便于与 Go 包布局一一对照。
#[path = "glue.rs"]
pub mod glue;

// 对外扁平导出 Glue 实现符号，调用方不必写 gluetikv::glue::。
pub use glue::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "glue_test.rs"]
mod glue_test;
