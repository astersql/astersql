// Copyright 2026 AsterSQL.

// 规划器会话扩展（plannersession）crate 根。
//
// 提供 `PlanCtxExtended` 等会话级规划扩展：空值拒绝检查表达式上下文、
// 只读用户变量映射，以及事务预热（AdviseTxnWarmup）委托。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 会话扩展上下文与相关 trait / 错误类型。
mod context;
pub use context::*;
