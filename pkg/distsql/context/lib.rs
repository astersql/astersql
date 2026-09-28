// Copyright 2026 AsterSQL.

// DistSQL 执行上下文 crate 的入口模块。
//
// DistSQL（Distributed SQL）负责把 SQL 下推到 TiKV/TiFlash 等存储节点执行。
// 本 crate 封装会话侧传入的运行时上下文 `DistSQLContext`（并发、副本读、
// 资源组、告警、内存跟踪、TiFlash 配置等），并重新导出依赖命名空间以对齐 Go 包路径。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 错误构造工具（对齐 Go `errors` 包）。
pub mod errors {
    pub use errctx_dependency::errors::*;
}

/// 告警（Warning）追加与处理器工具；`WarnAppenderRef` 为可跨线程共享的追加器。
pub mod contextutil {
    pub use errctx_dependency::contextutil::*;
    /// 可跨线程共享的告警追加器引用。
    pub type WarnAppenderRef = std::sync::Arc<dyn WarnAppender + Send + Sync>;
}

/// 错误级别上下文（Error Context）：按错误组决定 Warn/Strict 等处理级别。
pub mod errctx {
    pub use errctx_dependency::errctx::*;
}

/// KV 客户端与副本读类型等存储层依赖。
pub mod kv {
    pub use kv_dependency::{Client, ReplicaReadType, ResourceGroupTagBuilder};
}

/// TiKV 请求变量（退避参数、Killed 信号等）。
pub mod tikvstore {
    pub use kv_dependency::Variables;
}

/// MySQL 协议优先级常量（High / NoPriority 等）。
pub mod mysql {
    pub use mysql_dependency::r#const::{HighPriority, NoPriority, PriorityEnum};
}

/// 会话内存 Tracker（跟踪查询内存配额与消耗）。
pub mod memory {
    pub use memory_dependency::tracker::{NewTracker, Tracker};
}

/// SQL CPU 用量统计（TiDB / TiKV 两侧耗时）。
pub mod ppcpuusage {
    pub use ppcpuusage_dependency::*;
}

/// SQLKiller：通过原子信号中断正在执行的查询。
pub mod sqlkiller {
    pub use sqlkiller_dependency::sqlkiller::SQLKiller;
}

/// 执行细节与 RU（Resource Unit，资源计量单位）v2 指标。
pub mod execdetails {
    pub use execdetails_dependency::execdetails::{RuntimeStatsColl, SyncExecDetails};
    pub use execdetails_dependency::ruv2_metrics::RUV2Metrics;
}

/// TiFlash（列存加速引擎）副本读策略与相关类型。
pub mod tiflash {
    pub use tiflash_dependency::*;
}

/// DistSQLContext 主体实现。
mod context;
pub use context::*;

#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
