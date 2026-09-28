// Copyright 2026 AsterSQL.

// 环形缓冲队列 crate 入口：导出 `queue` 模块并挂接单元/迁移测试。
//
// 对应 Go `pkg/util/queue`；对外再导出 `Queue`、`NewQueue` 等符号。

/// 队列实现模块。
pub mod queue;

pub use queue::*;

#[cfg(test)]
mod queue_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
