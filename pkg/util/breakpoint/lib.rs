// Copyright 2026 AsterSQL.

// breakpoint 工具包入口。
//
// 对应 Go `util/breakpoint`：对外汇出断点注入相关符号，并在测试配置下挂载迁移回归用例。
// 断点（breakpoint）配合故障注入（failpoint），在指定执行点触发会话上下文中的回调通知。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 会话上下文工具，用于在 session 上存取断点回调。
pub use contextutil_crate as contextutil;
/// 字符串工具依赖（与 Go 侧 stringutil 对齐）。
pub use stringutil_crate as stringutil;

/// 断点注入核心实现模块。
mod breakpoint;
/// 对外重新导出 `breakpoint` 模块中的公开 API。
pub use breakpoint::*;

/// 迁移期单元测试：校验 failpoint 启停与回调类型行为。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
