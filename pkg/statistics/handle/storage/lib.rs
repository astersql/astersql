// Copyright 2026 AsterSQL.

// 统计存储（storage）子 crate 入口。
//
// 聚合 GC、JSON 序列化、系统表读取、持久化写入、读写封装与增量更新等子模块，
// 并在测试配置下挂接对应单元测试。

#![allow(dead_code)]

/// 统计元数据垃圾回收（GC）。
pub mod gc;
/// 统计 JSON 导入/导出与分块编解码。
pub mod json;
/// 从 `mysql.stats_*` 系统表读取直方图、TopN、FM Sketch 等。
pub mod read;
/// 将统计写入系统表。
pub mod save;
/// 读写统一封装（对应 Go stats_read_writer）。
pub mod stats_read_writer;
/// 增量统计更新路径。
pub mod update;

pub use gc::*;
pub use json::*;
pub use read::*;
pub use save::*;
pub use stats_read_writer::*;
pub use update::*;

#[cfg(test)]
/// Dump/Load JSON 与分区统计相关测试（含 Go 草稿归档）。
mod dump_test;
#[cfg(test)]
/// GC 行为相关测试（含 Go 草稿归档）。
mod gc_test;
#[cfg(test)]
/// JSON 编解码边界与损坏载荷的单元测试。
mod json_aster_unit_test;
#[cfg(test)]
/// 从存储加载直方图相关测试（含 Go 草稿归档）。
mod read_test;
#[cfg(test)]
/// 统计持久化写入与 Go 路径的对抗一致性测试。
mod save_test;
#[cfg(test)]
/// stats_read_writer 相关测试。
mod stats_read_writer_test;
#[cfg(test)]
/// Incremental statistics update parity tests.
mod update_test;
