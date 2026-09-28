// Copyright 2026 AsterSQL.

// Standby（热备）包入口：导出空闲连接监视与 keyspace 加载控制器。
//
// Standby 指实例先以待命状态启动，收到激活（activate）请求后再绑定 keyspace
// （逻辑命名空间）并对外提供服务；本模块汇总对外可复用的类型与常量。

#![allow(dead_code)]

/// 空闲连接监视：超时无活跃流量时触发优雅退出。
pub mod idle_watcher;
/// Standby 状态机、HTTP 路由与 Manager 回收通知。
pub mod standby;

/// 再导出空闲监视配置与启动入口。
pub use idle_watcher::{IdleWatcherConfig, start_idle_watcher};
/// 再导出 standby 控制器、状态常量与退出选项解析。
pub use standby::{
    ACTIVATED_STATE, ActivateRequest, DEFAULT_CLOSE_CONNECTION_WAIT, ExitOptions, ExitSignal,
    ExitSignaler, HTTP_PATH_PREFIX, LoadKeyspaceController, MANAGER_FREE_MAX_ATTEMPTS,
    MAX_CLOSE_CONNECTION_WAIT, ManagerClient, RecordingExitSignaler, STANDBY_STATE, State,
    TERMINATING_STATE, TIDB_NORMAL_RESTART_LOG_PATH, parse_exit_options, parse_exit_wait,
};

/// 经典部署模式下的 standby 行为测试。
#[cfg(test)]
#[path = "standby_test.rs"]
mod standby_test;

/// NextGen/starter 模式下的 standby 行为测试。
#[cfg(test)]
#[path = "standby_nextgen_test.rs"]
mod standby_nextgen_test;

/// 空闲连接监视器与 Go 事务状态判定的一致性测试。
#[cfg(test)]
#[path = "idle_watcher_test.rs"]
mod idle_watcher_test;
