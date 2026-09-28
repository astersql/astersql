// Copyright 2026 AsterSQL.

// 调度协程池（spool）：可阻塞/非阻塞提交、容量调谐与池管理器注册。
//
// 对应 Go 侧 `resourcemanager/pool/spool`，在资源管理器调度下动态调整并发度，
// 并通过 `poolmanager` 登记任务通道；选项模块控制是否阻塞等待空闲槽位。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]
extern crate self as astersql_resourcemanager_spool;
/// 复用上级 `basepool`：名称、任务 ID 与调谐时间戳。
#[path = "../basepool.rs"]
pub mod pool;
/// 池管理器：登记/查找命名池与任务通道。
#[path = "../../poolmanager/lib.rs"]
pub mod poolmanager;
/// 资源管理器依赖再导出（调度组件、工具类型等）。
pub mod resourcemanager {
    pub use resourcemanager_dependency::*;
}
/// 工具类型再导出（如 Component、UNKNOWN 等）。
pub mod util {
    pub use resourcemanager_dependency::util::*;
}
/// 池构造选项（如是否 blocking）。
mod option;
pub use option::*;
/// spool 核心：`Pool` 的 Run / Tune / Release 等逻辑。
mod spool;
pub use spool::*;

#[cfg(test)]
mod main_test;
/// 迁移对照单测：选项、构造注册、非阻塞容量、并发与调谐。
#[cfg(test)]
mod migration_aster_unit_test;
/// spool 行为补充测试。
#[cfg(test)]
mod spool_test;
