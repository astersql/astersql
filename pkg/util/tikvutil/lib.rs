// Copyright 2026 AsterSQL.
// TiKV 相关工具包入口。
//
// 对应 Go `pkg/util/tikvutil`。再导出提交并发度等原子变量，
// 供 TiDB 与 TiKV 交互路径读取系统变量 `tidb_committer_concurrency`。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// TiKV 工具实现模块。
pub mod tikvutil;
/// 再导出 `tikvutil` 模块的全部公开符号。
pub use tikvutil::*;

/// AsterSQL 迁移补充单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
