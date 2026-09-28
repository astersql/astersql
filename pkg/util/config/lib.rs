// Copyright 2026 AsterSQL.

// `util/config` crate 入口。
//
// 导出 Plan Replayer 配置加载 API，并重导出依赖的 `logutil` 与 `variable`。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 本 crate 自引用别名，供测试模块按 Go 包名风格引用。
extern crate self as util_config;

/// 后台日志工具。
pub use logutil;
/// 系统变量与会话变量定义。
pub use variable;

/// Plan Replayer 配置加载实现。
mod config;
/// 再导出 config 模块全部公开符号。
pub use config::*;

/// AsterSQL 迁移补充单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
