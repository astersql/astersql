// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Lifetime hooks for a daemon application.
//!
//! Corresponds to Go `br/pkg/streamhelper/daemon/interface.go`.
//! 守护进程生命周期钩子与 owner 管理抽象；Rust 用 `CancellationToken`
//! 模拟 Go `context.Context`，避免直接依赖 etcd/kvproto。

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

/// Error type used by daemon hooks and owner management.
/// 守护钩子与 owner 竞选共用的错误类型（仅携带消息字符串）。
#[derive(Debug)]
pub struct DaemonError {
    message: String,
}

impl DaemonError {
    /// 从任意可转 String 的值构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for DaemonError {}

/// 本包内统一 Result 别名，错误固定为 `DaemonError`。
pub type Result<T> = std::result::Result<T, DaemonError>;

/// Cancellation-aware context, matching Go `context.Context` usage in this package.
/// 可取消上下文：包装 `CancellationToken`，语义对齐 Go context。
#[derive(Clone, Debug)]
pub struct Context {
    token: CancellationToken,
}

impl Context {
    /// Background context that is never cancelled by default.
    /// 根上下文：默认永不取消，相当于 Go `context.Background()`。
    pub fn background() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    /// Create a child context cancelled when `cancel` is invoked or the parent ends.
    /// 派生子上下文；父取消或调用返回的 `CancelFunc` 均会取消子树。
    pub fn with_cancel(parent: &Context) -> (Context, CancelFunc) {
        let token = parent.token.child_token();
        let cancel = CancelFunc {
            token: token.clone(),
        };
        (Self { token }, cancel)
    }

    /// Whether this context has been cancelled.
    /// 是否已取消（非阻塞探测）。
    pub fn is_done(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Wait until this context is cancelled.
    /// 异步等待取消完成，供 select/分支使用。
    pub fn cancelled(&self) -> impl Future<Output = ()> + '_ {
        self.token.cancelled()
    }

    /// Underlying cancellation token (for select branches).
    /// 暴露底层 token，便于与其它 Future 组合。
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Explicitly cancel this context (used by tests / outer shutdown).
    /// 显式取消；测试与外层关停路径使用。
    pub fn cancel(&self) {
        self.token.cancel();
    }
}

/// Cancel function returned by [`Context::with_cancel`], matching Go `context.CancelFunc`.
/// 可克隆的幂等取消句柄：与 Go `CancelFunc` 一样可重复或并发调用。
#[derive(Clone, Debug)]
pub struct CancelFunc {
    token: CancellationToken,
}

impl CancelFunc {
    /// Cancel the associated child context.
    /// 取消关联的子上下文。
    pub fn call(&self) {
        self.token.cancel();
    }
}

/// Interface describes the lifetime hook of a daemon application.
/// 守护应用生命周期钩子：启动、成为 owner、周期 tick、名称追踪。
pub trait Interface: Send {
    /// OnStart start the service whatever the tidb-server is owner or not.
    /// 进程启动即调用，与是否 owner 无关。
    fn OnStart(&mut self, ctx: &Context);

    /// OnBecomeOwner would be called once become the owner.
    /// The context passed in would be canceled once it is no more the owner.
    /// 成为 owner 时调用；传入的 ctx 在失去 owner 时会被取消。
    fn OnBecomeOwner(&mut self, ctx: Context);

    /// OnTick would be called periodically.
    /// The error can be recorded.
    /// 周期回调；返回的错误可被上层记录但不一定终止守护。
    fn OnTick(&mut self, ctx: &Context) -> Result<()>;

    /// Name returns the name which is used for tracing the daemon.
    /// 返回守护名称，用于日志与追踪。
    fn Name(&self) -> String;
}

/// Owner election / lease manager subset used by [`crate::owner_daemon::OwnerDaemon`].
///
/// Local trait (slim crate): avoids pulling `astersql-owner` / etcd / kvproto.
/// Methods mirror the Go `owner.Manager` surface actually used by this package.
/// Owner 选举/租约管理子集，供 `OwnerDaemon` 使用；方法对齐 Go `owner.Manager`
/// 实际用到的表面，避免引入完整 etcd 依赖。
pub trait Manager: Send + Sync {
    /// ID returns the ID of the manager.
    /// 管理器实例 ID（竞选身份标识）。
    fn ID(&self) -> String;

    /// IsOwner returns whether this manager is the owner.
    /// 当前是否持有 owner。
    fn IsOwner(&self) -> bool;

    /// CampaignOwner campaigns the owner (may start background election).
    /// 发起/继续竞选，可能启动后台选举循环。
    fn CampaignOwner(&self) -> Result<()>;

    /// ForceToBeOwner restarts election trying to become the owner.
    /// 强制重启选举以争取成为 owner。
    fn ForceToBeOwner(&self, ctx: &Context) -> Result<()>;

    /// RetireOwner makes the manager no longer the owner.
    /// 主动卸任 owner。
    fn RetireOwner(&self);
}

/// Shared manager handle.
/// 共享的 Manager 句柄（`Arc<dyn Manager>`）。
pub type SharedManager = Arc<dyn Manager>;
