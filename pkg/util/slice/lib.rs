// Copyright 2026 AsterSQL.

// 切片工具 crate 入口。
//
// 对应 Go `pkg/util/slice`：导出切片相关辅助 API，并挂接单元/迁移测试。

/// 切片工具实现模块。
pub mod slice;

pub use slice::*;

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod slice_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
