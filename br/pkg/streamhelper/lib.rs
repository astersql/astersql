// Copyright 2026 AsterSQL.

//! 日志备份 streamhelper 库入口（对应 Go `br/pkg/streamhelper`）。
//!
//! 组装 region 扫描、元数据客户端、flush 订阅、checkpoint 推进器与环境抽象；
//! `pub use` 扁平导出公开符号，保持与 Go 同包调用习惯一致。
//! 测试文件仅在 `cfg(test)` 下 path 挂载，避免进入发布 API。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::all
)]

/// 外部依赖桩（etcd/存储等精简替身）。
#[path = "stubs.rs"]
pub mod stubs;

/// 任务/检查点等模型与键编码。
#[path = "models.rs"]
pub mod models;

/// Region 迭代与 Store 拓扑。
#[path = "regioniter.rs"]
pub mod regioniter;

/// 前缀扫描工具。
#[path = "prefix_scanner.rs"]
pub mod prefix_scanner;

/// 元数据客户端（任务 CRUD、暂停等）。
#[path = "client.rs"]
pub mod client;

/// 检查点收集器。
#[path = "collector.rs"]
pub mod collector;

/// TiKV flush 事件订阅与拓扑适配。
#[path = "flush_subscriber.rs"]
pub mod flush_subscriber;

/// 推进器运行环境与 TiKV 配置读取。
#[path = "advancer_env.rs"]
pub mod advancer_env;

/// Advancer 对 MetaDataClient 的扩展操作。
#[path = "advancer_cliext.rs"]
pub mod advancer_cliext;

/// 推进器与 owner daemon 的粘合。
#[path = "advancer_daemon.rs"]
pub mod advancer_daemon;

/// CheckpointAdvancer 核心逻辑。
#[path = "advancer.rs"]
pub mod advancer;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "prefix_scanner_test.rs"]
mod prefix_scanner_test;

#[cfg(test)]
#[path = "models_test.rs"]
mod models_test;

#[cfg(test)]
#[path = "basic_lib_for_test.rs"]
mod basic_lib_for_test;

#[cfg(test)]
#[path = "export_test.rs"]
mod export_test;

#[cfg(test)]
#[path = "regioniter_test.rs"]
mod regioniter_test;

#[cfg(test)]
#[path = "subscription_test.rs"]
mod subscription_test;

#[cfg(test)]
#[path = "flush_subscriber_test.rs"]
mod flush_subscriber_test;

#[cfg(test)]
#[path = "integration_test.rs"]
mod integration_test;

#[cfg(test)]
#[path = "client_test.rs"]
mod client_test;

#[cfg(test)]
#[path = "advancer_test.rs"]
mod advancer_test;

#[cfg(test)]
#[path = "advancer_cliext_test.rs"]
mod advancer_cliext_test;

#[cfg(test)]
#[path = "advancer_daemon_test.rs"]
mod advancer_daemon_test;

#[cfg(test)]
#[path = "advancer_env_test.rs"]
mod advancer_env_test;

#[cfg(test)]
#[path = "collector_test.rs"]
mod collector_test;

// 以下 re-export 对齐 Go 包级符号，供 cmd/br 与其它 crate 直接使用。
pub use advancer::{
    CheckpointAdvancer, NewCheckpointAdvancer, NewCommandCheckpointAdvancer,
    NewTiDBCheckpointAdvancer, isScanLockLockedError, lowerResolveLockMaxVersion,
    newCheckpointWithSpan, newCheckpointWithTS, resolveLockMaxVersionMaxRetry,
    resolveLockRetryLowerBound, resolveLockRetryLowerBoundLag, resolveLockTargetUpperBound,
    resolveLocksForRangeWithMaxVersionRetry,
};
pub use advancer_cliext::*;
pub use advancer_daemon::*;
pub use advancer_env::{
    Env, GetLogBackupFlushIntervalFromTiKVConfig, LogBackupFlushIntervalGetter, RegionLockResolver,
    StreamMeta, dialTimeOut, logBackupSafePointTTL, logBackupServiceID,
    parseLogBackupFlushIntervalFromConfig,
};
pub use client::*;
pub use collector::*;
pub use flush_subscriber::*;
pub use models::*;
pub use prefix_scanner::*;
pub use regioniter::*;
pub use stubs::*;
