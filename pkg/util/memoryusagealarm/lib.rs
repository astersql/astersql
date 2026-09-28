// Copyright 2026 AsterSQL.

// 内存使用告警（memory usage alarm）crate 入口。
//
// 定期检测进程/系统内存是否逼近 OOM（Out Of Memory，内存耗尽）风险，
// 超阈值时落盘 SQL 快照与 profile。对外重导出 `memoryusagealarm` 模块。

#[path = "memoryusagealarm.rs"]
pub mod memoryusagealarm;

pub use memoryusagealarm::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "memoryusagealarm_test.rs"]
mod memoryusagealarm_test;
