// Copyright 2026 AsterSQL.

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

//! BR operator 子包入口：对齐 Go `br/pkg/task/operator`。
//! 聚合运维型子命令实现（checksum、CRR checkpoint、force-flush、migration 等），
//! 并通过 `pub use` 平铺导出，供 `br/cmd/br/operator.rs` 注册 CLI。
//! `stubs`/`config` 先声明，供其余模块依赖；测试仅在 `cfg(test)` 挂载。
//! `prepare_snap`：快照准备运维入口。
//! `base64ify`：调试用编码辅助。
//! `test_storage`：单测存储替身，勿用于生产。
//! 各子模块保持与 Go 文件名一一对应，便于对照。

#[path = "stubs.rs"]
pub mod stubs;

#[path = "config.rs"]
pub mod config;

#[path = "base64ify.rs"]
pub mod base64ify;

#[path = "checksum_table.rs"]
pub mod checksum_table;

#[path = "crr_checkpoint.rs"]
pub mod crr_checkpoint;

#[path = "force_flush.rs"]
pub mod force_flush;

#[path = "list_migration.rs"]
pub mod list_migration;

#[path = "migrate_to.rs"]
pub mod migrate_to;

#[path = "prepare_snap.rs"]
pub mod prepare_snap;

#[path = "test_storage.rs"]
pub mod test_storage;

// 与 Go 同包导出习惯一致：调用方写 `operator::RunForceFlush` 即可。
pub use base64ify::*;
pub use checksum_table::*;
pub use config::*;
pub use crr_checkpoint::*;
pub use force_flush::*;
pub use list_migration::*;
pub use migrate_to::*;
pub use prepare_snap::*;
pub use test_storage::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "crr_checkpoint_test.rs"]
mod crr_checkpoint_test;

#[cfg(test)]
#[path = "base64ify_test.rs"]
mod base64ify_test;

#[cfg(test)]
#[path = "test_storage_test.rs"]
mod test_storage_test;
