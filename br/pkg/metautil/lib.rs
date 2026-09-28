// Copyright 2026 AsterSQL.

//! `br/pkg/metautil` crate 入口：汇聚备份元数据读写、调试解码与统计文件辅助模块。
//! 模块挂载顺序与 Go 包文件职责对应：`debug`/`load`/`metafile`/`statsfile` 为生产路径，
//! `stubs` 提供 darwin arm64 下避免拉齐完整 kvproto 依赖的本地替身。
//! 测试模块仅在 `cfg(test)` 下挂载，保持与 Go `*_test.go` 的一一对照。
//! 对外 `pub use` 再导出各子模块公开 API，调用方只需依赖本 crate 根即可。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 本地桩：Message/CipherInfo 等，避免测试机强依赖完整 protobuf 栈。
#[path = "stubs.rs"]
pub mod stubs;

pub use stubs::kvproto;
pub use stubs::{
    DecodeTableID, JSONTable, Key, PartitionStatisticLoadTask, StatsReadWriter, StatsTypesJSONTable,
};

// 调试：把 backupmeta / stats 叶子解码为 jsons 旁路文件。
#[path = "debug.rs"]
pub mod debug;

// 加载：把 BackupMeta 中的 schema 聚合成 Database 映射。
#[path = "load.rs"]
pub mod load;

// 核心：v1/v2 元数据读写、加密与兼容性检查。
#[path = "metafile.rs"]
pub mod metafile;

// 统计文件：BackupStats / downloadStats 与 Go statsfile.go 对齐。
#[path = "statsfile.rs"]
pub mod statsfile;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "debug_test.rs"]
mod debug_test;

#[cfg(test)]
#[path = "load_test.rs"]
mod load_test;

#[cfg(test)]
#[path = "metafile_test.rs"]
mod metafile_test;

#[cfg(test)]
#[path = "statsfile_test.rs"]
mod statsfile_test;

#[cfg(test)]
#[path = "statsfile_test_support.rs"]
mod statsfile_test_support;

// 与 Go 包对外符号面一致：调用方 `use metautil::*` 即可拿到读写入口。
pub use debug::*;
pub use load::*;
pub use metafile::*;
pub use statsfile::*;
