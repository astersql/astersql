// Copyright 2026 AsterSQL.

// 会话管理器（sessmgr）包入口。
//
// 负责进程列表（ProcessInfo）、连接管理与内部会话协调相关依赖的 re-export，
// 并导出 `processinfo` 中的 Manager / ProcessInfo 等核心类型。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_session_sessmgr;

/// 鉴权与用户身份相关类型 re-export。
pub mod auth {
    pub use auth_dependency::parser::auth::auth::*;
}
/// 游标状态相关类型 re-export。
pub mod cursor {
    pub use cursor_dependency::*;
}
/// 磁盘资源跟踪相关类型 re-export。
pub mod disk {
    pub use disk_dependency::*;
}
/// 执行详情（execdetails）re-export。
pub mod execdetails {
    pub use execdetails_dependency::execdetails::*;
}
/// 元数据锁（MDL）定义 re-export。
pub mod mdldef {
    pub use mdldef_dependency::*;
}
/// 内存 Tracker re-export。
pub mod memory {
    pub use memory_dependency::tracker::*;
}
/// MySQL 协议常量（命令字、服务器状态位等）re-export。
pub mod mysql {
    pub use mysql_dependency::r#const::*;
}
/// SQL CPU 用量统计 re-export。
pub mod ppcpuusage {
    pub use ppcpuusage_dependency::*;
}
/// 资源组相关类型 re-export。
pub mod resourcegroup {
    pub use resourcegroup_dependency::*;
}
/// 语句上下文（StmtCtx）re-export。
pub mod stmtctx {
    pub use stmtctx_dependency::*;
}
/// 事务信息摘要 re-export。
pub mod txninfo {
    pub use txninfo_dependency::txn_info::*;
}

/// 进程列表与会话 Manager 实现。
pub mod processinfo;
pub use processinfo::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "processinfo_test.rs"]
mod processinfo_test;
