// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! OwnerDaemon: run a stateless daemon only on the TiDB owner node.
//!
//! Corresponds to Go `br/pkg/streamhelper/daemon/owner_daemon.go`.
//! Uses a local [`Manager`] trait (slim crate) instead of `pkg/owner`.
//!
//! Go's `OwnerDaemon` is documented as synchronous (no concurrent field access).
//! The returned loop still runs on another goroutine in tests while only the
//! shared `Manager` is touched externally — Rust mirrors that with interior
//! mutability so the loop future can be `'static` / spawned.
//!
//! 中文要点：仅在集群 owner 上跑无状态守护逻辑；`Begin` 负责竞选与
//! `OnStart`，真正的 tick 循环由返回的 [`DaemonLoop`] 异步执行。失主时
//! 通过 `cancel` 取消 become-owner 上下文，避免双主并发推进。
//! 选举细节下沉到 [`Manager`]，本文件只编排生命周期回调顺序。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::{debug, info, warn};

use crate::interface::{CancelFunc, Context, Interface, Manager, Result, SharedManager};

/// 可变状态：业务 daemon 与“当前是否以 owner 身份运行”的取消句柄。
struct OwnerDaemonState {
    daemon: Box<dyn Interface>,
    /// When `Some`, implies the daemon is running as owner.
    /// `Some` 表示已调用 `OnBecomeOwner`，可用该 cancel 触发失主清理。
    cancel: Option<CancelFunc>,
}

/// OwnerDaemon is a wrapper for running "daemon" in the TiDB cluster.
/// Generally, it uses the etcd election API (wrapped in the `Manager` interface),
/// and shares nothing between nodes.
/// Please make sure the daemon is "stateless" (i.e. it doesn't depend on the local storage or memory state.)
/// This struct is "synchronous" (which means there are no race accessing of these variables.).
///
/// 包装选举 Manager 与业务 Interface：节点间不共享本地内存状态。
/// Go 文档要求同步访问字段；Rust 用 `Mutex` 换取可 spawn 的 `'static` 循环。
pub struct OwnerDaemon {
    /// 与 DaemonLoop 共享，供 tick 与 Running 查询。
    state: Arc<Mutex<OwnerDaemonState>>,
    manager: SharedManager,
    /// 主循环轮询间隔，对齐 Go `tickInterval`。
    tickInterval: Duration,
}

/// New creates a new owner daemon.
/// 构造时尚不竞选；`cancel` 为空表示尚未成为 owner。
pub fn New(
    daemon: Box<dyn Interface>,
    manager: SharedManager,
    tickInterval: Duration,
) -> OwnerDaemon {
    OwnerDaemon {
        state: Arc::new(Mutex::new(OwnerDaemonState {
            daemon,
            cancel: None,
        })),
        manager,
        tickInterval,
    }
}

impl OwnerDaemon {
    /// Running tests whether the daemon is running (i.e. is it the owner?)
    /// 以 `cancel.is_some()` 判定，而非直接读 Manager，避免竞态窗口误解。
    pub fn Running(&self) -> bool {
        self.state.lock().unwrap().cancel.is_some()
    }

    /// 失主路径：取消 become-owner 子上下文，清空 cancel 标志。
    fn cancelRun(state: &mut OwnerDaemonState) {
        if state.cancel.is_some() {
            let name = state.daemon.Name();
            info!(daemon = %name, "cancel running daemon");
            if let Some(cancel) = state.cancel.take() {
                cancel.call();
            }
        }
    }

    /// 仍是 owner 时的单次 tick：首次进入会 `OnBecomeOwner`，随后必调 `OnTick`。
    fn ownerTick(state: &mut OwnerDaemonState, manager: &dyn Manager, ctx: &Context) {
        // If not running, switching to running.
        // 从非 owner 切到 owner：派生可取消上下文交给业务层。
        if state.cancel.is_none() {
            let (cx, cancel) = Context::with_cancel(ctx);
            state.cancel = Some(cancel);
            info!(
                id = %manager.ID(),
                daemon_id = %state.daemon.Name(),
                "daemon became owner"
            );
            // Note: maybe save the context so we can cancel the tick when we are not owner?
            state.daemon.OnBecomeOwner(cx);
        }

        // Tick anyway.
        // OnTick 失败只记日志，不中断循环（与 Go Warn 后继续一致）。
        if let Err(err) = state.daemon.OnTick(ctx) {
            // Go: log.Warn("failed on tick", logutil.ShortError(err))
            warn!(error = %err, "failed on tick");
        }
    }

    /// Begin starts the daemon.
    /// It would do some bootstrap task, and return a closure that would begin the main loop.
    /// 顺序：CampaignOwner → OnStart → 返回可 spawn 的 DaemonLoop。
    /// 竞选失败则不会调用 OnStart（与 Go 错误短路一致）。
    pub fn Begin(&self, ctx: Context) -> Result<DaemonLoop> {
        let name = {
            let state = self.state.lock().unwrap();
            state.daemon.Name()
        };
        info!(daemon_id = %name, "begin advancer daemon");
        self.manager.CampaignOwner()?;

        // start the service.
        {
            let mut state = self.state.lock().unwrap();
            state.daemon.OnStart(&ctx);
        }

        // Go constructs time.NewTicker here, so the schedule starts during Begin
        // and an invalid interval panics only after OnStart has completed.
        assert!(
            !self.tickInterval.is_zero(),
            "non-positive interval for NewTicker"
        );
        let firstTick = tokio::time::Instant::now() + self.tickInterval;

        Ok(DaemonLoop {
            state: Arc::clone(&self.state),
            manager: Arc::clone(&self.manager),
            tickInterval: self.tickInterval,
            firstTick,
            ctx,
        })
    }

    /// 强制成为 owner，透传给 Manager（测试与运维路径）。
    pub fn ForceToBeOwner(&self, ctx: &Context) -> Result<()> {
        self.manager.ForceToBeOwner(ctx)
    }

    /// 主动卸任；是否立即失主取决于 Manager 实现。
    /// 不直接改 `cancel`，由后续 tick 的非 owner 分支触发 `cancelRun`。
    pub fn RetireIfOwner(&self) {
        self.manager.RetireOwner();
    }

    /// Shared manager handle (for tests / external RetireOwner like Go).
    /// 暴露共享 Manager，便于测试直接 Retire / 观察竞选状态。
    pub fn manager(&self) -> SharedManager {
        Arc::clone(&self.manager)
    }
}

/// Main loop returned by [`OwnerDaemon::Begin`], matching Go's `func()`.
/// 持有与 OwnerDaemon 共享的 state/manager，可在独立任务中 `run`。
pub struct DaemonLoop {
    state: Arc<Mutex<OwnerDaemonState>>,
    manager: SharedManager,
    tickInterval: Duration,
    /// First deadline is captured by Begin, matching time.NewTicker construction.
    firstTick: tokio::time::Instant,
    ctx: Context,
}

impl DaemonLoop {
    /// Run the owner tick loop until `ctx` is cancelled.
    /// 对齐 Go ticker：丢弃 tokio interval 的立即首次触发；非 owner 走 cancelRun。
    pub async fn run(self) {
        info!(
            id = %self.manager.ID(),
            daemon_id = %self.state.lock().unwrap().daemon.Name(),
            "begin running daemon"
        );
        let mut tick = tokio::time::interval_at(self.firstTick, self.tickInterval);
        // Go's ticker drops missed sends. Skip likewise advances to the next
        // scheduled deadline rather than replaying every missed interval.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = self.ctx.cancelled() => {
                    info!(
                        id = %self.manager.ID(),
                        daemon_id = %self.state.lock().unwrap().daemon.Name(),
                        "daemon loop exits"
                    );
                    return;
                }
                _ = tick.tick() => {
                    // 先读 IsOwner 再锁 state，缩短持锁时间并匹配 Go 分支语义。
                    let is_owner = self.manager.IsOwner();
                    debug!(
                        is_owner,
                        daemon_id = %self.state.lock().unwrap().daemon.Name(),
                        "daemon tick start"
                    );
                    let mut state = self.state.lock().unwrap();
                    if is_owner {
                        OwnerDaemon::ownerTick(&mut state, self.manager.as_ref(), &self.ctx);
                    } else {
                        OwnerDaemon::cancelRun(&mut state);
                    }
                }
            }
        }
    }
}

/// Helper to build a shared manager from a concrete type.
/// 把具体 Manager 装箱为 `SharedManager`，供 New / 测试复用。
pub fn share_manager<M: Manager + 'static>(manager: M) -> SharedManager {
    Arc::new(manager)
}
