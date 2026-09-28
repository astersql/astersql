// Copyright 2026 AsterSQL.

// 系统变量定义（vardef）crate 入口。
//
// 聚合 MySQL/TiDB 系统变量名常量、默认值、作用域/类型标志，以及运行时租约等全局状态。
// 对应 Go `pkg/sessionctx/vardef`；供 `variable` 等上层在 SET/SHOW 时引用。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_sessionctx_vardef;

// MySQL 兼容系统变量名与 SET NAMES/CHARSET 相关常量。
mod sysvar;
pub use sysvar::*;
// TiDB 专有系统变量名、默认值、原子全局状态与辅助转换。
mod tidb_vars;
pub use tidb_vars::*;
// Schema/Stats/PlanReplayer 等运行时租约，以及 NextGen 只读变量判断。
mod runtime;
pub use runtime::*;

// 下列为迁移期单元测试与 Go 行为对照测试。
#[cfg(test)]
#[path = "runtime_1_aster_unit_test.rs"]
mod runtime_aster_unit_test;
#[cfg(test)]
#[path = "runtime_test.rs"]
mod runtime_test;
#[cfg(test)]
#[path = "tidb_vars_2_aster_unit_test.rs"]
mod tidb_vars_aster_unit_test;
#[cfg(test)]
#[path = "tidb_vars_test.rs"]
mod tidb_vars_test;
