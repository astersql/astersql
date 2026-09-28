// Copyright 2026 AsterSQL.

// 容量与类型大小常量 crate 入口。
//
// 对应 Go `pkg/util/size`：导出二进制容量单位（KB/MB/…）以及常见 Go 类型
// 头部大小常量，供内存追踪（memory trace）估算使用。

/// 容量/类型大小常量实现模块。
mod size;
pub use size::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
