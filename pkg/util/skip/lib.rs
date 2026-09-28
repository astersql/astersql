// Copyright 2026 AsterSQL.

// 测试跳过辅助 crate 入口。
//
// 对应 Go `pkg/util/skip`：在 `-short` / `-long` 等测试标志下按条件跳过用例；
// 对外再导出 `skip` 模块 API，并提供 `testkit` 以便访问 `testflag`。

/// 跳过逻辑实现模块。
pub mod skip;
pub use skip::*;

/// 测试工具再导出：暴露 `testflag` 供 NotUnderLong 等读取 `-long`。
pub mod testkit {
    pub use testflag;
}

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
