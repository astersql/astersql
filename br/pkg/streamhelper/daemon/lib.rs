// Copyright 2026 AsterSQL.

//! 日志备份 owner 守护进程子 crate 入口。
//!
//! 对应 Go `br/pkg/streamhelper/daemon`：把选举抽象（`interface`）与
//! 仅在 owner 节点运行的循环（`owner_daemon`）拼成对外 API。
//! 测试模块在 `cfg(test)` 下按 path 挂载，避免污染生产导出。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

/// 守护进程生命周期与选举 Manager 抽象。
#[path = "interface.rs"]
pub mod interface;

/// OwnerDaemon 实现：竞选、tick、失主取消。
#[path = "owner_daemon.rs"]
pub mod owner_daemon;

// 对外扁平导出，调用方无需记子模块路径（与 Go 同包符号一致）。
pub use interface::*;
pub use owner_daemon::*;

#[cfg(test)]
#[path = "owner_daemon_test.rs"]
mod owner_daemon_test;

#[cfg(test)]
#[path = "interface_test.rs"]
mod interface_test;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
