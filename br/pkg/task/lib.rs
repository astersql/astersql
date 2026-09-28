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

//! BR 任务层 crate 根：对齐 Go `br/pkg/task` 包入口。
//! 职责是挂载备份/恢复/流备份子模块，并以 `pub use` 平铺导出符号，
//! 供 `br/cmd` 与其它 crate 以 `task::*` 方式直接引用，避免深层路径。
//! 子模块按业务切分：common/encryption 为共享配置与密钥；
//! backup* 覆盖全量、EBS、raw、txn；restore* 覆盖数据与元数据恢复；
//! stream 对应日志备份任务编排。测试模块仅在 `cfg(test)` 下挂载。
//! 本文件无业务逻辑，只负责模块图与可见性；改导出顺序不影响运行时。
//! stubs 提供跨平台桩，保证 arm64 无 kv/domain 时仍可编译任务层。

// 生产子模块：顺序与 Go package 文件拆分一致，无运行时初始化副作用。
// stubs 须先于其它模块，避免循环依赖与路径解析失败。

#[path = "stubs.rs"]
pub mod stubs;

// 加密与密钥派生，被 backup/restore 共用。
#[path = "encryption.rs"]
pub mod encryption;

// 公共 flag/配置与任务胶水。
#[path = "common.rs"]
pub mod common;

// 全量备份主路径。
#[path = "backup.rs"]
pub mod backup;

// EBS 卷备份专用编排。
#[path = "backup_ebs.rs"]
pub mod backup_ebs;

// Raw KV 备份。
#[path = "backup_raw.rs"]
pub mod backup_raw;

// 事务一致性备份。
#[path = "backup_txn.rs"]
pub mod backup_txn;

// 恢复总控。
#[path = "restore.rs"]
pub mod restore;

// 数据文件恢复。
#[path = "restore_data.rs"]
pub mod restore_data;

// EBS 元数据恢复。
#[path = "restore_ebs_meta.rs"]
pub mod restore_ebs_meta;

// Raw KV 恢复。
#[path = "restore_raw.rs"]
pub mod restore_raw;

// 事务一致性恢复。
#[path = "restore_txn.rs"]
pub mod restore_txn;

// 日志备份（stream）任务。
#[path = "stream.rs"]
pub mod stream;

// 单元/对齐测试：与同名 Go `*_test.go` 对照，不进入发布二进制。

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "backup_ebs_test.rs"]
mod backup_ebs_test;

#[cfg(test)]
#[path = "backup_test.rs"]
mod backup_test;

#[cfg(test)]
#[path = "backup_raw_test.rs"]
mod backup_raw_test;

#[cfg(test)]
#[path = "backup_txn_test.rs"]
mod backup_txn_test;

#[cfg(test)]
#[path = "common_test.rs"]
mod common_test;

#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;

#[cfg(test)]
#[path = "encryption_test.rs"]
mod encryption_test;

#[cfg(test)]
#[path = "restore_nokit_test.rs"]
mod restore_nokit_test;

#[cfg(test)]
#[path = "restore_data_test.rs"]
mod restore_data_test;

#[cfg(test)]
#[path = "restore_ebs_meta_test.rs"]
mod restore_ebs_meta_test;

#[cfg(test)]
#[path = "restore_test.rs"]
mod restore_test;

#[cfg(test)]
#[path = "restore_raw_test.rs"]
mod restore_raw_test;

#[cfg(test)]
#[path = "restore_txn_test.rs"]
mod restore_txn_test;

#[cfg(test)]
#[path = "stream_test.rs"]
mod stream_test;

// 对外 re-export：保持与 Go 同包可见性习惯，调用方无需写 `task::backup::...`。
// 通配导出含桩与实现，测试与 CLI 均可直接引用公开符号。
pub use backup::*;
pub use backup_ebs::*;
pub use backup_raw::*;
pub use backup_txn::*;
pub use common::*;
pub use encryption::*;
pub use restore::*;
pub use restore_data::*;
pub use restore_ebs_meta::*;
pub use restore_raw::*;
pub use restore_txn::*;
pub use stream::*;
pub use stubs::*;
