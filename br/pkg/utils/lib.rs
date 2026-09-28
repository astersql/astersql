// Copyright 2026 AsterSQL.

//! `br/pkg/utils` crate 入口：汇总备份/恢复共用工具子模块并再导出高频 API。
//! 模块声明用显式 `#[path]`，避免目录结构与 Go 包布局不一致时解析失败。
//! 测试子模块仅在 `cfg(test)` 下挂载，保持发布产物不编入单测。
//! 子模块按 Go `br/pkg/utils` 职责切分：重试、过滤、进度、schema、存储连接等。
//! `pub use` 聚合高频符号，减少调用方深层路径依赖。
//! allow 属性放宽命名以贴近 Go 导出风格（非蛇形）。
//! stubs 提供最小 kvproto/KeyRange 替身，保证 crate 可独立编译。
//! encryption/json/key 等为纯工具，无全局可变状态。
//! worker/wait 提供并发与同步原语，供备份流水线使用。
//! register/progress 面向运维可观测性。
//! store_manager 管理到 TiKV 的 gRPC 连接池。
//! 测试文件与实现同目录，经 path 属性挂入，避免与源码同文件混放。
//! 修改导出列表时需评估下游 crate 的破坏性变更。
//! 本入口不包含业务主流程，只做模块编排与再导出。
//! 与 Go 包一一对应的子文件应优先查阅同名 `.go` 语义。
//! dyn_pprof_* 按目标 OS 选用，避免非 Unix 链接失败。
//! pointer/misc/common 为薄辅助，保持无副作用。
//! 完成注释任务时不得调整 mod 顺序以免干扰审查 diff。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 桩类型与 kvproto 替身，供尚未完全链入上游依赖时编译。
#[path = "stubs.rs"]
pub mod stubs;

pub use stubs::kvproto;
pub use stubs::{KeyRange, KvKey};

// 退避策略，供重试与客户端限速。
#[path = "backoff.rs"]
pub mod backoff;

/// 全局 ID / Next-Gen 兼容性等公共辅助。
// 通用杂项与共享小工具。
#[path = "common.rs"]
pub mod common;

/// TiDB 配置读写与日志备份任务计数。
// 数据库名/连接相关辅助。
#[path = "db.rs"]
pub mod db;

// 动态 pprof：非 Unix 与 Unix 分文件实现。
/// 非 POSIX 平台空实现，保持符号可链接。
#[path = "dyn_pprof_other.rs"]
pub mod dyn_pprof_other;

/// Unix 信号驱动的动态 pprof 监听。
#[path = "dyn_pprof_unix.rs"]
pub mod dyn_pprof_unix;

#[cfg(all(test, unix))]
#[path = "dyn_pprof_unix_test.rs"]
mod dyn_pprof_unix_test;

/// 备份加密密钥与 cipher 辅助。
// 备份内容解密与有效加密方法判定。
#[path = "encryption.rs"]
pub mod encryption;

/// 备份错误分类与重试决策。
// 备份错误分类与重试/放弃策略。
#[path = "error_handling.rs"]
pub mod error_handling;

/// PiTR 库表 ID 过滤跟踪器。
// PiTR 库表跟踪与过滤。
#[path = "filter.rs"]
pub mod filter;

// 备份元数据 JSON 编解码（对齐 Go json.go）。
#[path = "json.rs"]
pub mod json;

// 键解析、区间相交与日期格式化。
#[path = "key.rs"]
pub mod key;

/// 进程内存上限与告警配置挂载。
// 进程内存告警配置与监视启动。
#[path = "memory_monitor.rs"]
pub mod memory_monitor;

/// 杂项字符串/时长/环境辅助。
// 其它零散工具函数。
#[path = "misc.rs"]
pub mod misc;

/// Option/指针零值读取。
// 指针/Option 取值辅助。
#[path = "pointer.rs"]
pub mod pointer;

/// status/pprof HTTP 监听启动。
// 性能剖析入口封装。
#[path = "pprof.rs"]
pub mod pprof;

/// 进度条与吞吐汇报。
// 进度条与日志进度报告。
#[path = "progress.rs"]
pub mod progress;

/// BR 进程向 etcd/PD 的注册。
// 任务注册与元信息登记。
#[path = "register.rs"]
pub mod register;

/// 带退避的重试封装。
// 通用重试循环与致命错误放弃。
#[path = "retry.rs"]
pub mod retry;

/// 系统库名、临时库前缀与引号规则。
// 系统库/临时库命名与引号处理。
#[path = "schema.rs"]
pub mod schema;

/// TiKV store 连接池/管理器。
// TiKV store gRPC 连接池管理。
#[path = "store_manager.rs"]
pub mod store_manager;

/// 条件轮询等待（取消/超时）。
// 条件等待封装。
#[path = "wait.rs"]
pub mod wait;

/// worker token 通道与 panic 捕获。
// worker token 通道与 panic 捕获。
#[path = "worker.rs"]
pub mod worker;

// 对外再导出：调用方可用 `astersql_br_pkg_utils::X` 直接访问常用符号。
pub use backoff::BackoffStrategy;
pub use error_handling::{
    ErrorContext, ErrorHandlingResult, ErrorHandlingStrategy, HandleBackupError,
    HandleUnknownBackupError, MessageIsRetryableStorageError, NewDefaultContext, NewErrorContext,
    NewZeroRetryContext, contextCancelledMsg, messageIsCredentialNotFoundError,
    messageIsNotFoundStorageError, messageIsPermissionDeniedStorageError,
};
pub use filter::{MatchSchema, MatchTable, NewPiTRIdTracker, PiTRIdTracker};
pub use pointer::GetOrZero;
pub use retry::{
    FallBack2CreateTable, GiveUpRetryOn, VerboseRetry, WithRetry, WithRetryReturnLastErr,
    WithRetryV2,
};
pub use schema::{
    EncloseDBAndTable, EncloseName, GetSysDBCIStrName, IsSysDB, IsSysOrTempSysDB, IsTemplateSysDB,
    NeedAutoID, StripTempDBPrefix, StripTempDBPrefixIfNeeded, TemporaryDBName, UnquoteName,
};
pub use wait::WaitUntil;
pub use worker::{
    AsyncStreamBy, BuildWorkerTokenChannel, CatchAndLogPanic, DefaultWorkerTokenChannelSize,
    MaxWorkerTokenChannelSize, PanicToErr, Result as WorkerResult, WorkerTokenChannel,
};

// 以下测试模块与 Go 同名文件一一对应，仅编译期挂载。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "wait_test.rs"]
mod wait_test;

#[cfg(test)]
#[path = "worker_test.rs"]
mod worker_test;

#[cfg(test)]
#[path = "backoff_test.rs"]
mod backoff_test;

#[cfg(test)]
#[path = "common_test.rs"]
mod common_test;

#[cfg(test)]
#[path = "db_test.rs"]
mod db_test;

#[cfg(test)]
#[path = "error_handling_test.rs"]
mod error_handling_test;

#[cfg(test)]
#[path = "filter_test.rs"]
mod filter_test;

#[cfg(test)]
#[path = "json_test.rs"]
mod json_test;

#[cfg(test)]
#[path = "key_test.rs"]
mod key_test;

#[cfg(test)]
#[path = "memory_monitor_test.rs"]
mod memory_monitor_test;

#[cfg(test)]
#[path = "misc_test.rs"]
mod misc_test;

#[cfg(test)]
#[path = "progress_test.rs"]
mod progress_test;

#[cfg(test)]
#[path = "pprof_test.rs"]
mod pprof_test;

#[cfg(test)]
#[path = "register_test.rs"]
mod register_test;

#[cfg(test)]
#[path = "retry_test.rs"]
mod retry_test;

#[cfg(test)]
#[path = "schema_test.rs"]
mod schema_test;

#[cfg(test)]
#[path = "store_manager_test.rs"]
mod store_manager_test;
