// Copyright 2026 AsterSQL.

// 磁盘临时存储与用量跟踪工具包。
//
// 对应 Go `pkg/util/disk`：导出临时目录初始化/清理（`tempDir`）以及磁盘用量
// `Tracker` 别名（复用 memory 包实现）。执行器 spill（内存不足时落盘）等场景依赖此包。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

/// 全局配置转发，供临时存储路径等读取。
pub mod config {
    pub use astersql_config::*;
}
/// 内存包转发，供磁盘 `Tracker` 复用同一套跟踪实现。
pub mod memory {
    pub use tidb_memory::*;
}

pub mod tempDir;
pub mod tracker;
pub(crate) use tempDir::checkTempDirExist;
pub use tempDir::*;
pub use tracker::*;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "tempDir_test.rs"]
mod temp_dir_test;
