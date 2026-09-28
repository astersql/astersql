// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/streamhelper/daemon` vs Go.
//!
//! Covers normal owner tick flow, ownership loss cleanup, CampaignOwner errors,
//! and OnTick error recording (loop continues).
//!
//! 中文：对照 Go 公开契约的集中回归——正常成为 owner、失主清理、
//! 竞选失败短路、OnTick 出错不退出循环、Force/Retire 透传。
//! 与 `owner_daemon_test` 互补：本文件偏契约边界，对方偏端到端时序。
//! Mock 可注入竞选错误、关闭再夺主，覆盖 Begin 短路与 Retire 计数。
//! 所有等待均用短步进轮询，避免依赖真实时钟抖动。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::interface::{Context, DaemonError, Interface, Manager, Result};
use crate::owner_daemon::{New, share_manager};

/// Minimal mock matching Go `owner.NewMockManager` behaviour used by daemon tests.
/// 可注入竞选错误、统计 Force/Retire，并可选延迟再夺主。
struct MockManager {
    /// Manager::ID 返回值。
    id: String,
    /// 当前 owner 标志。
    is_owner: AtomicBool,
    /// 非空时 CampaignOwner 直接失败。
    campaign_err: Mutex<Option<String>>,
    /// ForceToBeOwner 是否被调用过。
    force_called: AtomicBool,
    /// RetireOwner 调用次数。
    retire_count: AtomicU32,
    /// After retire, become owner again after this delay (Go mock re-campaigns ~1s; tests use shorter).
    /// `None` 表示卸任后不再自动夺回。
    reown_after: Mutex<Option<Duration>>,
}

impl MockManager {
    /// 默认开启 200ms 再夺主。
    fn new(id: &str) -> Arc<Self> {
        Arc::new(Self {
            id: id.to_string(),
            is_owner: AtomicBool::new(false),
            campaign_err: Mutex::new(None),
            force_called: AtomicBool::new(false),
            retire_count: AtomicU32::new(0),
            // Must outlast daemon tick (tests use 30–50ms) so cancelRun observes !IsOwner.
            // 默认 200ms，大于测试 tick，保证能观察到失主窗口。
            reown_after: Mutex::new(Some(Duration::from_millis(200))),
        })
    }

    /// 设置/清除竞选错误注入。
    fn set_campaign_err(&self, msg: Option<&str>) {
        *self.campaign_err.lock().unwrap() = msg.map(str::to_string);
    }
}

/// Arc 包装以实现 Manager。
struct MockManagerArc(Arc<MockManager>);

impl Manager for MockManagerArc {
    /// 透传固定 id。
    fn ID(&self) -> String {
        self.0.id.clone()
    }

    /// 读原子 owner 标志。
    fn IsOwner(&self) -> bool {
        self.0.is_owner.load(Ordering::SeqCst)
    }

    fn CampaignOwner(&self) -> Result<()> {
        // 优先返回注入错误，模拟 etcd 竞选失败。
        if let Some(err) = self.0.campaign_err.lock().unwrap().clone() {
            return Err(DaemonError::new(err));
        }
        self.0.is_owner.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn ForceToBeOwner(&self, _ctx: &Context) -> Result<()> {
        // 记录调用次数语义用布尔标志即可。
        self.0.force_called.store(true, Ordering::SeqCst);
        self.0.is_owner.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn RetireOwner(&self) {
        self.0.retire_count.fetch_add(1, Ordering::SeqCst);
        self.0.is_owner.store(false, Ordering::SeqCst);
        let delay = *self.0.reown_after.lock().unwrap();
        // 可选异步再夺主，对齐 Go mock campaign 循环。
        if let Some(delay) = delay {
            let mgr = Arc::clone(&self.0);
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                mgr.is_owner.store(true, Ordering::SeqCst);
            });
        }
    }
}

/// RecordingApp 计数器：用于断言生命周期事件次数。
#[derive(Default)]
struct AppState {
    /// OnStart 是否发生。
    service_start: bool,
    /// 当前是否在 become-owner 会话。
    begun: bool,
    /// OnTick 累计次数。
    tick_count: u32,
    /// OnBecomeOwner 累计次数。
    become_count: u32,
    /// 失主清理累计次数。
    stop_count: u32,
    /// 非空时 OnTick 返回该错误（循环应继续）。
    tick_err: Option<String>,
}

/// 可观测的测试 daemon，记录 start/become/tick/stop。
struct RecordingApp {
    inner: Arc<Mutex<AppState>>,
}

impl Interface for RecordingApp {
    /// 仅翻转 service_start。
    fn OnStart(&mut self, _ctx: &Context) {
        self.inner.lock().unwrap().service_start = true;
    }

    fn OnBecomeOwner(&mut self, ctx: Context) {
        {
            let mut st = self.inner.lock().unwrap();
            // 禁止同一会话双重 become。
            assert!(!st.begun, "failed: an app is started twice");
            st.begun = true;
            st.become_count += 1;
        }
        // 失主 cancel 后标记 stop。
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            ctx.cancelled().await;
            let mut st = inner.lock().unwrap();
            st.begun = false;
            st.stop_count += 1;
        });
    }

    fn OnTick(&mut self, _ctx: &Context) -> Result<()> {
        let mut st = self.inner.lock().unwrap();
        assert!(st.begun, "failed: an app is ticking before start");
        st.tick_count += 1;
        // 错误不中断外层循环，由 OwnerDaemon 记日志。
        if let Some(err) = st.tick_err.clone() {
            return Err(DaemonError::new(err));
        }
        Ok(())
    }

    /// 固定测试名。
    fn Name(&self) -> String {
        "testing".to_string()
    }
}

/// 轮询等待条件，对齐 Go Eventually。
/// 超时后仍做最后一次求值，避免刚好踩边界漏检。
async fn wait_until(cond: impl Fn() -> bool, timeout: Duration, step: Duration) -> bool {
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        tokio::time::sleep(step).await;
    }
    cond()
}

/// Go creates `time.NewTicker` inside `Begin`, before returning the loop closure.
/// If the caller delays starting that closure past the first deadline, the first
/// owner tick must therefore be ready immediately rather than waiting a fresh
/// interval from `run`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tick_schedule_starts_during_begin() {
    let ctx = Context::background();
    let mgr = MockManager::new("delayed_loop_start");
    let state = Arc::new(Mutex::new(AppState::default()));
    let loop_fn = New(
        Box::new(RecordingApp {
            inner: Arc::clone(&state),
        }),
        share_manager(MockManagerArc(mgr)),
        Duration::from_millis(150),
    )
    .Begin(ctx.clone())
    .expect("Begin should succeed");

    tokio::time::sleep(Duration::from_millis(225)).await;
    let handle = tokio::spawn(async move { loop_fn.run().await });

    assert!(
        wait_until(
            || state.lock().unwrap().tick_count > 0,
            Duration::from_millis(75),
            Duration::from_millis(5),
        )
        .await,
        "ticker deadline must be measured from Begin, not run"
    );

    ctx.cancel();
    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("daemon loop should exit on ctx cancel")
        .expect("loop task join");
}

/// Go calls `time.NewTicker` synchronously in `Begin`, where a non-positive
/// duration panics after `OnStart` has run. Rust `Duration` cannot be negative,
/// so zero is the representable boundary case.
#[test]
fn zero_tick_interval_panics_during_begin() {
    let mgr = MockManager::new("zero_interval");
    let state = Arc::new(Mutex::new(AppState::default()));
    let daemon = New(
        Box::new(RecordingApp {
            inner: Arc::clone(&state),
        }),
        share_manager(MockManagerArc(mgr)),
        Duration::ZERO,
    );

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = daemon.Begin(Context::background());
    }))
    .expect_err("zero interval must panic during Begin");
    let message = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic");
    assert!(message.contains("non-positive interval for NewTicker"));
    assert!(
        state.lock().unwrap().service_start,
        "Go invokes OnStart before constructing the invalid ticker"
    );
}

/// 单测聚合多个 Go/Rust 契约场景，避免重复搭脚手架。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn go_rust_public_contract_matches() {
    // --- normal: Begin -> OnStart -> owner tick -> OnBecomeOwner -> OnTick ---
    // 正常路径：Begin 同步 OnStart，循环内成为 owner 后才 Running/OnTick。
    let ctx = Context::background();
    let mgr = MockManager::new("owner_daemon_test");
    let state = Arc::new(Mutex::new(AppState::default()));
    let d = New(
        Box::new(RecordingApp {
            inner: Arc::clone(&state),
        }),
        share_manager(MockManagerArc(Arc::clone(&mgr))),
        Duration::from_millis(50),
    );

    // Begin 之前服务未启动。
    assert!(!state.lock().unwrap().service_start);
    let loop_fn = d.Begin(ctx.clone()).expect("Begin should succeed");
    // Begin 成功后 OnStart 必已执行。
    assert!(
        state.lock().unwrap().service_start,
        "OnStart must run during Begin"
    );
    // mock 竞选应立即成功。
    assert!(
        mgr.is_owner.load(Ordering::SeqCst),
        "CampaignOwner should make mock the owner"
    );
    // Running 依赖 cancel 句柄，首个 ownerTick 前应为 false。
    assert!(!d.Running(), "Running is false until first ownerTick");

    // 独立任务跑主循环，模拟 Go 另起 goroutine。
    let handle = tokio::spawn(async move {
        loop_fn.run().await;
    });

    // 等待首次成为 owner。
    assert!(
        wait_until(
            || state.lock().unwrap().begun,
            Duration::from_secs(2),
            Duration::from_millis(10)
        )
        .await,
        "OnBecomeOwner should fire"
    );
    // Running 与 cancel 句柄绑定。
    assert!(
        wait_until(
            || d.Running(),
            Duration::from_secs(2),
            Duration::from_millis(10)
        )
        .await,
        "Running should be true after becoming owner"
    );
    // 至少发生一次 OnTick。
    assert!(
        wait_until(
            || state.lock().unwrap().tick_count > 0,
            Duration::from_secs(2),
            Duration::from_millis(10)
        )
        .await,
        "OnTick should fire while owner"
    );

    // --- boundary / resource cleanup: lose ownership cancels become-owner ctx ---
    // 失主：cancel become 上下文，begun 清零，Running 变 false。
    MockManagerArc(Arc::clone(&mgr)).RetireOwner();
    assert!(!mgr.is_owner.load(Ordering::SeqCst));
    assert!(
        wait_until(
            || !state.lock().unwrap().begun && state.lock().unwrap().stop_count > 0,
            Duration::from_secs(2),
            Duration::from_millis(10)
        )
        .await,
        "owner ctx cancel should clear begun (resource cleanup)"
    );
    assert!(
        wait_until(
            || !d.Running(),
            Duration::from_secs(2),
            Duration::from_millis(10)
        )
        .await,
        "Running should clear after cancelRun"
    );

    // Snapshot before re-campaigning can race with the daemon tick. Once the
    // manager reports ownership, the loop may already have become owner and
    // ticked before the waiter resumes.
    // 快照计数避免与异步 tick 竞态误判。
    let ticks_before = state.lock().unwrap().tick_count;
    let become_before = state.lock().unwrap().become_count;
    // Go mock re-campaigns; wait until owner again then tick.
    // 等待 mock 再夺主，并确认 become/tick 相对快照增长。
    assert!(
        wait_until(
            || mgr.is_owner.load(Ordering::SeqCst),
            Duration::from_secs(2),
            Duration::from_millis(10)
        )
        .await,
        "mock should re-own after retire"
    );
    assert!(
        wait_until(
            || {
                let st = state.lock().unwrap();
                st.begun && st.become_count > become_before && st.tick_count > ticks_before
            },
            Duration::from_secs(2),
            Duration::from_millis(10)
        )
        .await,
        "should become owner and tick again"
    );

    // ForceToBeOwner forwards to manager.
    // Force 应触达 Manager 标志位。
    d.ForceToBeOwner(&ctx).unwrap();
    assert!(mgr.force_called.load(Ordering::SeqCst));

    // 取消上下文后循环应退出。
    ctx.cancel();
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("daemon loop should exit on ctx cancel")
        .expect("loop task join");

    // --- error: CampaignOwner failure prevents OnStart ---
    // 竞选失败必须短路，禁止 OnStart。
    let mgr2 = MockManager::new("campaign_fail");
    mgr2.set_campaign_err(Some("campaign failed"));
    let state2 = Arc::new(Mutex::new(AppState::default()));
    let d2 = New(
        Box::new(RecordingApp {
            inner: Arc::clone(&state2),
        }),
        share_manager(MockManagerArc(mgr2)),
        Duration::from_millis(50),
    );
    // Begin 应返回竞选错误字符串。
    let err = match d2.Begin(Context::background()) {
        Err(e) => e,
        Ok(_) => panic!("expected CampaignOwner error"),
    };
    assert!(err.to_string().contains("campaign failed"));
    // 失败路径不得副作用到 OnStart。
    assert!(
        !state2.lock().unwrap().service_start,
        "OnStart must not run when CampaignOwner fails"
    );

    // --- OnTick error is recorded; loop continues (Running stays true while owner) ---
    // OnTick 持续报错仍应多次 tick，且 Running 保持 true。
    let mgr3 = MockManager::new("tick_err");
    let state3 = Arc::new(Mutex::new(AppState {
        tick_err: Some("tick boom".to_string()),
        ..AppState::default()
    }));
    let ctx3 = Context::background();
    let d3 = New(
        Box::new(RecordingApp {
            inner: Arc::clone(&state3),
        }),
        share_manager(MockManagerArc(mgr3)),
        Duration::from_millis(30),
    );
    let loop3 = d3.Begin(ctx3.clone()).unwrap();
    let h3 = tokio::spawn(async move { loop3.run().await });
    // 至少两次 tick 证明错误被吞掉。
    assert!(
        wait_until(
            || state3.lock().unwrap().tick_count >= 2,
            Duration::from_secs(2),
            Duration::from_millis(10)
        )
        .await,
        "tick errors must not abort the loop"
    );
    // 仍是 owner 则 Running 保持。
    assert!(d3.Running());
    ctx3.cancel();
    tokio::time::timeout(Duration::from_secs(2), h3)
        .await
        .expect("tick-err loop exit")
        .expect("join");

    // RetireIfOwner forwards to manager.
    // RetireIfOwner 透传且关闭自动再夺主，便于精确计数。
    let mgr4 = MockManager::new("retire");
    mgr4.is_owner.store(true, Ordering::SeqCst);
    let d4 = New(
        Box::new(RecordingApp {
            inner: Arc::new(Mutex::new(AppState::default())),
        }),
        share_manager(MockManagerArc(Arc::clone(&mgr4))),
        Duration::from_millis(50),
    );
    // Disable reown for this assertion.
    // 关闭 reown，断言卸任后保持非 owner。
    *mgr4.reown_after.lock().unwrap() = None;
    d4.RetireIfOwner();
    assert!(!mgr4.is_owner.load(Ordering::SeqCst));
    assert_eq!(mgr4.retire_count.load(Ordering::SeqCst), 1);
}
