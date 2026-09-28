// Copyright 2026 AsterSQL.

// 规划器上下文（planctx）crate 根：依赖再导出与 PlanContext 相关 API。
//
// 对应 TiDB `planner/planctx`，向上层规划器提供会话/表达式/InfoSchema/KV 等
// 接口的统一入口；`context` 模块定义 Common / PlanContext 与 BuildPBContext。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as planctx_crate;

/// 会话侧值存储与警告追加器（WarnAppender）。
pub mod contextutil {
    pub use contextutil_crate::context::ValueStoreContext;
    pub use contextutil_crate::{NewStaticWarnHandler, WarnAppender};
}
/// 表达式求值/构建上下文。
pub mod exprctx {
    pub use exprctx_crate::*;
}
/// InfoSchema（元数据只读视图）相关类型。
pub mod infoschema {
    pub use infoschema_crate::*;
}
/// KV 存储与客户端抽象。
pub mod kv {
    pub use kv_crate::*;
}
/// 模型侧 TableItemID 等标识。
pub mod model {
    pub use model_crate::group_4::TableItemID;
}
/// Ranger（范围推导）上下文。
pub mod rangerctx {
    pub use rangerctx_crate::*;
}
/// 会话管理器接口。
pub mod sessmgr {
    pub use sessmgr_crate::*;
}
/// SQL 执行器接口（含受限执行器）。
pub mod sqlexec {
    pub use sqlexec_crate::*;
}
/// 表锁只读上下文。
pub mod tablelock {
    pub use tablelock_crate::*;
}
/// 会话变量 SessionVars。
pub mod variable {
    pub use variable_crate::session::SessionVars;
}

/// PlanContext / BuildPBContext 等核心定义。
mod context;
pub use context::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// Detach 浅拷贝与 EmptyPlanContextExtended 的迁移单元测试。
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "context_test.rs"]
/// BuildPBContext::Detach 与 Go TestContextDetach 对齐的测试。
mod context_test;
