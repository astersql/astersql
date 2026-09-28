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

//! Go-equivalent tests for `br/pkg/streamhelper/daemon/owner_daemon_test.go`.
//!
//! Maps `TestDaemon` → `test_daemon`. Owner election is mocked at the
//! `Manager` boundary (Go: `owner.NewMockManager`); daemon tick / become /
//! retire semantics exercise the real `OwnerDaemon` implementation.
//!
//! 中文：验证 Begin→OnStart→成为 owner→OnTick→卸任清理→再次成为 owner 的
//! 完整生命周期；选举边界用 MockManager，业务回调用 AnApp 探针。
//! 时间尺度刻意压缩：daemon tick 100ms、再夺主 200ms，使 1s Eventually 仍可靠。
//! 信号层用 tokio watch 代替 Go `chan struct{}`，语义保持“关闭即唤醒”。
//! 本文件不测真实 etcd；任何选举时序都由 mock 决定。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tracing::info;

use crate::interface::{Context, Interface, Manager, Result};
use crate::owner_daemon::{New, share_manager};

/// Closeable oneshot signal matching Go `chan struct{}` + `close`.
/// 可关闭一次性信号：用 watch(bool) 模拟 Go channel close 语义。
#[derive(Clone)]
struct Signal {
    /// 发送端：close 时写入 true。
    tx: watch::Sender<bool>,
    /// 接收端：wait 轮询变更。
    rx: watch::Receiver<bool>,
}

impl Signal {
    /// 初始为未触发（false）。
    fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self { tx, rx }
    }

    /// 发送 true，唤醒所有 wait 方（对应 close(chan)）。
    fn close(&self) {
        let _ = self.tx.send(true);
    }

    /// 在超时内等到信号，否则断言失败并带上场景名。
    async fn wait(&self, timeout: Duration, what: &str) {
        let mut rx = self.rx.clone();
        if *rx.borrow() {
            return;
        }
        let ok = tokio::time::timeout(timeout, async {
            loop {
                if rx.changed().await.is_err() {
                    return;
                }
                if *rx.borrow_and_update() {
                    return;
                }
            }
        })
        .await
        .is_ok();
        assert!(ok, "{what} not triggered after {timeout:?}");
    }
}

/// Mock matching Go `owner.NewMockManager` behaviour used by `TestDaemon`:
/// `CampaignOwner` becomes owner immediately (Go campaign goroutine's first
/// `toBeOwner`); after `RetireOwner`, the campaign loop re-owns following its
/// sleep (Go ≈ 1s; shortened so `Eventually(..., 1s, ...)` stays reliable while
/// still outlasting one daemon tick).
///
/// 竞选立刻成功；卸任后延迟再夺回，延迟须大于 daemon tick 以便观察到失主。
struct MockManager {
    /// 节点标识，对应 Manager::ID。
    id: String,
    /// 当前是否自认为 owner。
    is_owner: AtomicBool,
    /// Delay before re-owning after `RetireOwner` (Go campaign tick ≈ 1s).
    /// 再夺主延迟；Go 约 1s，测试缩短到 200ms。
    reown_after: Duration,
}

impl MockManager {
    /// 构造默认延迟 200ms 的 mock。
    fn new(id: &str) -> Arc<Self> {
        Arc::new(Self {
            id: id.to_string(),
            is_owner: AtomicBool::new(false),
            // > daemon tick (100ms) so cancelRun observes !IsOwner before reown.
            // 必须大于 100ms tick，否则 cancelRun 可能看不到非 owner。
            reown_after: Duration::from_millis(200),
        })
    }
}

/// 把 Arc 包装成 Manager trait 对象，便于 share_manager。
struct MockManagerArc(Arc<MockManager>);

impl Manager for MockManagerArc {
    /// 返回固定测试 id。
    fn ID(&self) -> String {
        self.0.id.clone()
    }

    /// 读取原子 owner 标志。
    fn IsOwner(&self) -> bool {
        self.0.is_owner.load(Ordering::SeqCst)
    }

    fn CampaignOwner(&self) -> Result<()> {
        // Go: campaign goroutine hits `default` → `toBeOwner()` immediately.
        // 同步置 owner，跳过真实 etcd 选举。
        self.0.is_owner.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// 强制置 owner，忽略上下文。
    fn ForceToBeOwner(&self, _ctx: &Context) -> Result<()> {
        self.0.is_owner.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn RetireOwner(&self) {
        // Go: `UnsetOwner`; campaign loop later calls `toBeOwner` after sleep.
        // 先失主，再异步睡眠后夺回，模拟竞选循环。
        self.0.is_owner.store(false, Ordering::SeqCst);
        let mgr = Arc::clone(&self.0);
        let delay = mgr.reown_after;
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            mgr.is_owner.store(true, Ordering::SeqCst);
        });
    }
}

/// AnApp 内部可变状态：服务启动、owner 会话与探针信号。
struct AnAppState {
    /// Begin/OnStart 是否已执行。
    service_start: bool,
    /// 当前是否处于 OnBecomeOwner 会话中。
    begun: bool,
    /// 首次 tick 探针；失主后清空。
    ticking_messenger: Option<Signal>,
    /// 每个 owner 会话只关闭一次 tick 信号。
    ticking_once: bool,
    /// 失主清理完成探针。
    stop_messenger: Option<Signal>,
    /// 成为 owner 完成探针。
    start_messenger: Signal,
}

/// Test daemon application (Go: `anApp`).
/// 测试用 Interface：用信号暴露 start/tick/stop，供断言时序。
#[derive(Clone)]
struct AnApp {
    inner: Arc<Mutex<AnAppState>>,
}

/// 构造全空闲探针状态的 AnApp。
fn new_test_app() -> AnApp {
    AnApp {
        inner: Arc::new(Mutex::new(AnAppState {
            service_start: false,
            begun: false,
            ticking_messenger: None,
            ticking_once: false,
            stop_messenger: None,
            start_messenger: Signal::new(),
        })),
    }
}

impl Interface for AnApp {
    /// 标记服务已启动，供 Begin 前后断言。
    fn OnStart(&mut self, _ctx: &Context) {
        self.inner.lock().unwrap().service_start = true;
    }

    fn OnBecomeOwner(&mut self, ctx: Context) {
        let stop = Signal::new();
        let ticking = Signal::new();
        {
            let mut st = self.inner.lock().unwrap();
            // 同一会话不得双重 become（防双主逻辑回归）。
            assert!(!st.begun, "failed: an app is started twice");
            st.begun = true;
            st.ticking_messenger = Some(ticking);
            st.ticking_once = false;
            st.stop_messenger = Some(stop.clone());
        }

        // 监听 become-owner 上下文取消 → 清理 begun 并通知 stop。
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            ctx.cancelled().await;
            let mut st = inner.lock().unwrap();
            st.begun = false;
            st.ticking_messenger = None;
            // 重置 start 信号，便于再次成为 owner 后重新等待。
            st.start_messenger = Signal::new();
            stop.close();
        });

        self.inner.lock().unwrap().start_messenger.close();
    }

    fn OnTick(&mut self, _ctx: &Context) -> Result<()> {
        info!("tick");
        let mut st = self.inner.lock().unwrap();
        // tick 必须发生在 become 之后。
        assert!(st.begun, "failed: an app is ticking before start");
        if !st.ticking_once {
            if let Some(messenger) = st.ticking_messenger.clone() {
                info!("close");
                messenger.close();
            }
            st.ticking_once = true;
        }
        Ok(())
    }

    /// 固定名称，便于日志对照。
    fn Name(&self) -> String {
        "testing".to_string()
    }
}

impl AnApp {
    /// 断言 OnStart 是否已发生。
    fn assert_service(&self, service_start: bool) {
        let st = self.inner.lock().unwrap();
        assert_eq!(st.service_start, service_start);
    }

    /// 等待本轮 owner 会话的首次 OnTick。
    async fn assert_tick(&self, timeout: Duration) {
        let messenger = {
            let st = self.inner.lock().unwrap();
            st.ticking_messenger
                .clone()
                .expect("tickingMessenger must exist after become owner")
        };
        info!("waiting");
        messenger.wait(timeout, "tick").await;
    }

    /// 等待失主后 stop 信号（become 上下文被 cancel）。
    async fn assert_not_running(&self, timeout: Duration) {
        let messenger = {
            let st = self.inner.lock().unwrap();
            st.stop_messenger
                .clone()
                .expect("stopMessenger must exist after become owner")
        };
        messenger.wait(timeout, "stop").await;
    }

    /// 等待 OnBecomeOwner 关闭 start 信号。
    async fn assert_start(&self, timeout: Duration) {
        let messenger = {
            let st = self.inner.lock().unwrap();
            st.start_messenger.clone()
        };
        messenger.wait(timeout, "start").await;
    }
}

/// 轮询条件直至超时，对齐 Go `Eventually`。
async fn eventually(cond: impl Fn() -> bool, timeout: Duration, step: Duration) -> bool {
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        tokio::time::sleep(step).await;
    }
    cond()
}

/// Go: `TestDaemon` — service start, become owner, tick, retire, re-own, tick again.
/// 主场景：启动→tick→卸任清理→再夺主→再 tick→取消退出循环。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_daemon() {
    let ctx = Context::background();
    let app = new_test_app();
    let app_obs = app.clone();
    let ow = MockManager::new("owner_daemon_test");
    let d = New(
        Box::new(app),
        share_manager(MockManagerArc(Arc::clone(&ow))),
        Duration::from_millis(100),
    );

    // Begin 前不应 OnStart。
    app_obs.assert_service(false);
    let f = d.Begin(ctx.clone()).expect("Begin should succeed");
    app_obs.assert_service(true);
    let handle = tokio::spawn(async move {
        f.run().await;
    });

    app_obs.assert_start(Duration::from_secs(1)).await;
    app_obs.assert_tick(Duration::from_secs(1)).await;

    // 卸任后应观察到 stop，且 is_owner 瞬时为 false。
    MockManagerArc(Arc::clone(&ow)).RetireOwner();
    assert!(!ow.is_owner.load(Ordering::SeqCst));
    app_obs.assert_not_running(Duration::from_secs(1)).await;

    assert!(
        eventually(
            || ow.is_owner.load(Ordering::SeqCst),
            Duration::from_secs(1),
            Duration::from_millis(100),
        )
        .await,
        "mock manager should become owner again after RetireOwner"
    );
    // 再夺主后应重新 start 并再 tick。
    app_obs.assert_start(Duration::from_secs(1)).await;
    app_obs.assert_tick(Duration::from_secs(1)).await;

    ctx.cancel();
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("daemon loop should exit on ctx cancel")
        .expect("loop task join");
}
