// Copyright 2026 AsterSQL.

//! `astersql_br_pkg_checkpoint` crate 入口，对齐 Go 包 `br/pkg/checkpoint`。
//!
//! 装配顺序：先 `stubs`/`ticker` 提供边界与定时，再 `checkpoint` 核心 Runner，
//! 然后 `external_storage`/`storage` 落盘实现，最后 `backup`/`restore`/
//! `log_restore`/`manager` 场景适配。测试模块仅在 `cfg(test)` 挂载。
//! 对外 `pub use` 展平子模块符号，调用方无需写深层路径。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::all
)]

/// 依赖边界桩：Context/Storage/计时器等，避免强耦 PD/TiKV。
#[path = "stubs.rs"]
pub mod stubs;

/// 周期 tick 抽象，供 Runner 主循环驱动 flush/checksum/lock。
#[path = "ticker.rs"]
pub mod ticker;

/// CheckpointRunner 与分片/校验/遍历等核心算法。
#[path = "checkpoint.rs"]
pub mod checkpoint;

/// 基于外部对象存储的 checkpointStorage 与锁协议。
#[path = "external_storage.rs"]
pub mod external_storage;

/// 存储层辅助（与 external_storage 协作的路径/封装）。
#[path = "storage.rs"]
pub mod storage;

/// 备份场景路径常量与 Runner/元数据 API。
#[path = "backup.rs"]
pub mod backup;

/// 恢复场景 checkpoint 适配。
#[path = "restore.rs"]
pub mod restore;

/// 日志恢复（log restore）场景 checkpoint 适配。
#[path = "log_restore.rs"]
pub mod log_restore;

/// checkpoint 生命周期管理（启停/清理编排）。
#[path = "manager.rs"]
pub mod manager;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "checkpoint_test.rs"]
mod checkpoint_test;

#[cfg(test)]
#[path = "external_storage_test.rs"]
mod external_storage_test;

#[cfg(test)]
#[path = "storage_test.rs"]
mod storage_test;

#[cfg(test)]
#[path = "ticker_test.rs"]
mod ticker_test;

#[cfg(test)]
#[path = "log_restore_test.rs"]
mod log_restore_test;

#[cfg(test)]
#[path = "restore_test.rs"]
mod restore_test;

// 展平导出：保持与 Go 包级符号可见性相近。
pub use backup::*;
pub use checkpoint::*;
pub use external_storage::*;
pub use log_restore::*;
pub use manager::*;
pub use restore::*;
pub use storage::*;
pub use stubs::*;
pub use ticker::*;
