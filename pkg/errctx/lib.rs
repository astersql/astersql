// Copyright 2026 AsterSQL.

// `errctx` 包入口：错误处理上下文（Error Context）。
//
// 本 crate 对应 Go 的 `pkg/errctx`，为 SQL 执行路径提供统一的错误处置策略：
// 按错误分组（`ErrGroup`，如截断、空值、除零等）决定是直接返回错误、
// 降级为 warning（警告），还是忽略。具体策略实现见 `context.rs`。
//
// 本文件负责声明子模块、重导出依赖（errno / contextutil / errors），
// 以及在测试配置下挂载迁移单元测试与 `context` 测试。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

// 自引用别名：测试与 context 模块通过该路径引用本 crate。
extern crate self as astersql_errctx;

/// 重导出 MySQL / TiDB 错误码常量（对应 Go `errno` 包）。
pub use astersql_errno::errcode as errno;
/// 重导出警告追加工具（WarnHandler / WarnAppender 等）。
pub use contextutil_crate as contextutil;
/// 重导出共享错误类型与规范化辅助。
pub use contextutil_crate::{errors, warn};

/// 错误上下文核心实现（Level / ErrGroup / Context 等）。
#[path = "context.rs"]
pub mod errctx;

/// 测试用类型桩：提供溢出相关错误码别名。
#[cfg(test)]
pub mod types {
    pub use types_field::ErrOverflow;
}

/// Context API 单元测试。
#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;
/// Go→Rust 迁移行为对齐测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
