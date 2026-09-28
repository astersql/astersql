// Copyright 2026 AsterSQL.

// 会话上下文（session context）中的系统变量（sysvar）子系统 crate 入口。
//
// 系统变量控制会话/全局行为（如 autocommit、副本读模式等）。本模块聚合错误码、
// Mock 访问器、noop/已移除变量、序列状态、会话变量、慢日志规则、状态变量与
// TiDB 特有变量定义，并对齐 Go `sessionctx/variable` 包的导出面。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

// 自引用 crate 别名：供拆分的单测 crate/任务引用同一包符号。
extern crate self as astersql_sessionctx_variable;
extern crate self as task_variable;

/// 再导出配置、内核类型与 TiFlash 计算相关依赖。
pub use config;
pub use kerneltype;
pub use tiflashcompute;
pub use tls_dependency::tls as tlsutil;
/// 变量名/默认值常量定义（vardef）。
pub use vardef;

/// MySQL/TiDB 变量相关错误描述符。
pub mod error;
pub use error::{ErrIncorrectScope, ErrUnknownSystemVar};
/// 单测用全局变量访问器 Mock。
pub mod mock_globalaccessor;
/// next-gen（下一代）模式下受限变量校验与会话写入。
pub mod nextgen;
/// 仅 noop 实现的兼容性系统变量表。
pub mod noop;
/// 已从产品中移除的系统变量及移除原因。
pub mod removed;
pub use removed::CheckSysVarIsRemoved;
/// SEQUENCE 对象 last_value 等会话侧状态。
pub mod sequence_state;
/// 会话级变量、重试信息、用户变量与运行时过滤器。
pub mod session;
/// SET_VAR hint 对系统变量可更新性的影响标记。
pub mod setvar_affect;
/// 慢查询日志（slow log）规则解析。
pub mod slow_log;
/// 状态变量（status variable）定义与取值。
pub mod statusvar;
/// 系统变量元数据与校验逻辑。
pub mod sysvar;
/// 内建系统变量注册表。
pub mod sysvar_builtins;
pub use sysvar_builtins::*;

/// 核心 SessionVars 等会话变量结构。
mod variable;
pub use variable::*;
/// TiDB 特有会话/全局变量。
mod tidb_vars;
pub use tidb_vars::*;
/// 变量读写与转换工具函数。
mod varsutil;
pub use varsutil::*;

// 以下为按路径挂载的单元测试模块（仅 test cfg）。
#[cfg(test)]
#[path = "error_1_aster_unit_test.rs"]
mod error_1_aster_unit_test;
#[cfg(test)]
#[path = "error_test.rs"]
mod error_test;
#[cfg(test)]
#[path = "mock_globalaccessor_test.rs"]
mod mock_globalaccessor_test;
#[cfg(test)]
#[path = "nextgen_test.rs"]
mod nextgen_test;
#[cfg(test)]
#[path = "removed_test.rs"]
mod removed_test;
#[cfg(test)]
#[path = "session_hint_bridge_aster_unit_test.rs"]
mod session_hint_bridge_aster_unit_test;
#[cfg(test)]
#[path = "session_planner_ids_aster_unit_test.rs"]
mod session_planner_ids_aster_unit_test;
#[cfg(test)]
#[path = "slow_log_test.rs"]
mod slow_log_test;
#[cfg(test)]
#[path = "statusvar_2_aster_unit_test.rs"]
mod statusvar_2_aster_unit_test;
#[cfg(test)]
#[path = "statusvar_test.rs"]
mod statusvar_test;
#[cfg(test)]
#[path = "sysvar_3_aster_unit_test.rs"]
mod sysvar_3_aster_unit_test;
#[cfg(test)]
#[path = "sysvar_builtins_test.rs"]
mod sysvar_builtins_test;
#[cfg(test)]
#[path = "sysvar_test.rs"]
mod sysvar_test;
#[cfg(test)]
#[path = "tidb_vars_4_aster_unit_test.rs"]
mod tidb_vars_4_aster_unit_test;
#[cfg(test)]
#[path = "tidb_vars_test.rs"]
mod tidb_vars_test;
#[cfg(test)]
#[path = "variable_test.rs"]
mod variable_test;
#[cfg(test)]
#[path = "varsutil_test.rs"]
mod varsutil_test;
