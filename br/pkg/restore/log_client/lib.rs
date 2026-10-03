// Copyright 2026 AsterSQL.

//! 日志恢复客户端（log_client）crate 入口，对应 Go `br/pkg/restore/log_client`。
//! 职责：挂载 PITR/日志恢复所需子模块，并扁平再导出公开 API，
//! 使调用方无需深入子路径即可使用 Import、FileManager、SkipMap 等类型。
//! 测试模块以 `#[path]` 分文件挂载，实现与测试分离（Rust 约定）。
//! 模块声明顺序大致按依赖：桩 → 跳过表/迁移 → 文件管理 → 导入与客户端。
//! 再导出保持与 Go 包级可见符号一致，避免上层改写 import 路径。

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

// stubs：本地 Mem/桩，隔离 PD/TiKV/加密等外部依赖。
#[path = "stubs.rs"]
pub mod stubs;

// 检查点跳过位图：meta→group→file 三级索引。
#[path = "log_file_map.rs"]
pub mod log_file_map;

// SST/压缩相关类型，供迁移与导入路径复用。
#[path = "ssts.rs"]
pub mod ssts;

// 迁移构建器：把备份迁移元数据叠到文件遍历上。
#[path = "migration.rs"]
pub mod migration;

// 日志元数据遍历、TS 过滤与 KV 读取助手。
#[path = "log_file_manager.rs"]
pub mod log_file_manager;

// 已压缩文件的选取/跳过策略。
#[path = "compacted_file_strategy.rs"]
pub mod compacted_file_strategy;

// 按文件大小累积的 Region 分裂策略，含检查点跳过。
#[path = "log_split_strategy.rs"]
pub mod log_split_strategy;

// 导入重试与错误分类。
#[path = "import_retry.rs"]
pub mod import_retry;

// 日志文件导入器主体。
#[path = "import.rs"]
pub mod import;

// 对外 LogClient 编排入口。
#[path = "client.rs"]
pub mod client;

// PITR ID 映射文件名与块大小常量。
#[path = "id_map.rs"]
pub mod id_map;

// 批量元数据处理。
#[path = "batch_meta_processor.rs"]
pub mod batch_meta_processor;

// 扁平再导出：与 Go 包级符号可见性对齐，方便 restore 上层直接引用。
pub use batch_meta_processor::*;
pub use client::*;
pub use compacted_file_strategy::*;
// id_map 仅导出块大小与文件名常量，避免拖入内部细节。
pub use id_map::{PITRIdMapBlockSize, PitrIDMapsFilename};
pub use import::*;
pub use import_retry::*;
pub use log_file_manager::*;
pub use log_file_map::*;
pub use log_split_strategy::*;
pub use migration::*;
pub use ssts::*;

// 以下为测试专用挂载；cfg(test) 保证发布产物不含测试模块。
#[cfg(test)]
#[path = "export_test.rs"]
mod export_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "log_file_map_test.rs"]
mod log_file_map_test;

#[cfg(test)]
#[path = "migration_test.rs"]
mod migration_test;

#[cfg(test)]
#[path = "import_test.rs"]
mod import_test;

#[cfg(test)]
#[path = "import_retry_test.rs"]
mod import_retry_test;

#[cfg(test)]
#[path = "log_file_manager_test.rs"]
mod log_file_manager_test;

#[cfg(test)]
#[path = "client_test.rs"]
mod client_test;

#[cfg(test)]
#[path = "id_map_test.rs"]
mod id_map_test;

#[cfg(test)]
#[path = "compacted_file_strategy_test.rs"]
mod compacted_file_strategy_test;

#[cfg(test)]
#[path = "log_split_strategy_test.rs"]
mod log_split_strategy_test;

#[cfg(test)]
#[path = "ssts_test.rs"]
mod ssts_test;

#[path = "flow_control.rs"]
pub mod flow_control;
#[cfg(test)]
#[path = "flow_control_test.rs"]
mod flow_control_test;
