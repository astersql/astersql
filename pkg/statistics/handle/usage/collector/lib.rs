// Copyright 2026 AsterSQL.

// 统计使用量（usage）全局收集器 crate 入口。
//
// 再导出会话/全局收集器实现，并在测试配置下挂载与 Go 同路径的并发发送与刷盘用例。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 全局/会话增量收集器实现（双通道 + worker 合并）。
pub mod collector;
pub use collector::*;

#[cfg(test)]
#[path = "collector_test.rs"]
/// Go `collector_test.go` 同路径：SendDelta / SendDeltaSync 接受与 flush 语义。
mod collector_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移期补充单元测试：并行发送、Close 解除阻塞等。
mod migration_aster_unit_test;
