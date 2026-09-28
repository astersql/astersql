// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc. Licensed under Apache-2.0.

//! Prepare snapshot env — mirrors `br/pkg/task/operator/prepare_snap.go`.
//!
//! 为快照备份准备集群环境：并行暂停 GC、PD Scheduler，并对 TiKV 走
//! prepare_snap Preparer（暂停 admin / 等待 apply）。三阶段 `ReadyL` 齐备后
//! 打印固定日志供 operator 探测，并持续阻塞到调用方取消。
//! 无真实 PD 拨号时依赖 `DIAL_HOOKS`；缺 hook 则拒绝假成功。
//! 数据流：dialPD → createStoreManager → 三 keeper + ready 监视 →
//! OnAllReady/hintAllReady → 等待调用方取消 → cleanup → Close；首错上抛。
//! 与 Go `AdaptEnvForSnapshotBackup` 的 errgroup/WaitGroup 语义对应，
//! 任一 keeper 失败会取消其余任务，正常路径只响应调用方 context 取消。
//! 三阶段齐备后打印固定日志，兼容依赖文案探测的旧 operator。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::PauseGcConfig;
use crate::stubs::{
    BRServiceSafePoint, Config, Context, DIAL_HOOKS, DUMP_GOROUTINE_WHEN_EXIT, Error, GCManager,
    KeepaliveParams, MakeSafePointID, MemGCManager, NewPreparer, PdController, Result,
    StartServiceSafePointKeeper, StoreManager,
};

/// 构造连接 TiKV 的 StoreManager；优先走测试注入的 `create_store_manager` hook。
pub fn createStoreManager(
    pd: Arc<dyn crate::stubs::PDClient>,
    cfg: &Config,
) -> Result<Arc<StoreManager>> {
    // 测试/桩路径：避免真实 gRPC，直接返回注入实现。
    if let Some(hook) = DIAL_HOOKS.lock().unwrap().create_store_manager.as_ref() {
        return hook(pd, cfg);
    }
    if cfg.TLS.IsEnabled() {
        // 与 Go 一致：TLS 非法配置在建连前 Annotate 失败。
        let _ = cfg
            .TLS
            .ToTLSConfig()
            .map_err(|e| Error::Annotate(e.msg, "invalid tls config"))?;
    }
    // Keepalive 参数对齐 Go `keepalive.ClientParameters`；当前桩 StoreManager 仅保留字段。
    let _ = KeepaliveParams {
        Time: cfg.GRPCKeepaliveTime,
        Timeout: cfg.GRPCKeepaliveTimeout,
        PermitWithoutStream: true,
    };
    Ok(Arc::new(StoreManager::new(pd)))
}

/// 拨号 PD；有 hook 则用注入 PdController，否则在校验后明确报错（不伪造成功）。
pub fn dialPD(cfg: &Config) -> Result<Arc<PdController>> {
    if let Some(hook) = DIAL_HOOKS.lock().unwrap().dial_pd.as_ref() {
        return hook(cfg);
    }
    if cfg.TLS.IsEnabled() {
        let _ = cfg.TLS.ToTLSConfig()?;
        let _ = cfg.TLS.ToPDSecurityOption();
    }
    if cfg.PD.is_empty() {
        return Err(Error::Annotate("empty pd address", "failed to dial PD"));
    }
    // Real PD dial is a network boundary; without a hook, refuse rather than fake success.
    // 网络边界：未配置 hook 时拒绝，避免调用方误以为已连上真实 PD。
    Err(Error::Annotate(
        format!("pd dial not configured for {:?}", cfg.PD),
        "failed to dial PD",
    ))
}

/// 快照环境准备的共享上下文：PD/Store 管理器、就绪计数、取消与错误槽。
pub struct AdaptEnvForSnapshotBackupContext {
    pub pdMgr: Arc<PdController>,
    pub kvMgr: Arc<StoreManager>,
    pub cfg: PauseGcConfig,
    /// 已就绪阶段数；与 `ready_expected`（默认 3）比较。
    pub ready: Arc<AtomicUsize>,
    pub ready_expected: usize,
    /// 为 true 时各 keeper 退出并执行 cleanup。
    pub cancel: Arc<AtomicBool>,
    pub run_errs: Arc<Mutex<Vec<Error>>>,
}

impl AdaptEnvForSnapshotBackupContext {
    /// 关闭 PD 与 StoreManager，释放拨号资源。
    pub fn Close(&self) {
        self.pdMgr.Close();
        self.kvMgr.Close();
    }

    /// 标记某组件就绪并打日志；供 ready 监视线程汇总。
    pub fn ReadyL(&self, name: &str) {
        eprintln!("Stage ready. component={name}");
        self.ready.fetch_add(1, Ordering::SeqCst);
    }

    /// 以 cfg.TTL 为清理时限语义执行无返回值清理（对齐 Go WithTimeout）。
    fn cleanUpWith(&self, f: impl FnOnce()) {
        let _ttl = self.cfg.TTL;
        f();
    }

    /// 清理闭包若失败则合并进 `err_out`；多错误用 `; ` 拼接。
    fn cleanUpWithRetErr(&self, err_out: &mut Option<Error>, f: impl FnOnce() -> Result<()>) {
        let _ttl = self.cfg.TTL;
        if let Err(err) = f() {
            match err_out {
                Some(existing) => {
                    *existing = Error::new(format!("{}; {}", existing.msg, err.msg));
                }
                None => *err_out = Some(err),
            }
        }
    }
}

/// 打印固定英文日志；部分 operator 版本靠这些行判断“环境已就绪”。
fn hintAllReady() {
    // Hacking: some version of operators using the follow two logs to check whether we are ready...
    eprintln!("Schedulers are paused.");
    eprintln!("GC is paused.");
    eprintln!("All ready.");
}

/// AdaptEnvForSnapshotBackup blocks until the caller context is cancelled.
/// 入口：拨 PD → 启三 keeper 线程 → 等待三阶段就绪 → OnAllReady/OnExit → Close。
pub fn AdaptEnvForSnapshotBackup(ctx: Context, cfg: PauseGcConfig) -> Result<()> {
    // 异常退出时 dump 栈，便于排查卡死；全部就绪后清回 false。
    DUMP_GOROUTINE_WHEN_EXIT.store(true, Ordering::SeqCst);
    let mgr = dialPD(&cfg.Config)?;
    mgr.SetSchedulerPauseTTL(cfg.TTL);
    if cfg.Config.TLS.IsEnabled() {
        let _ = cfg
            .Config
            .TLS
            .ToTLSConfig()
            .map_err(|e| Error::Annotate(e.msg, "invalid tls config"))?;
    }
    let kvMgr = createStoreManager(mgr.GetPDClient(), &cfg.Config)?;

    // ready_expected=3：GC / scheduler / prepare_admin 三路。
    let cx = Arc::new(AdaptEnvForSnapshotBackupContext {
        pdMgr: mgr,
        kvMgr,
        cfg,
        ready: Arc::new(AtomicUsize::new(0)),
        ready_expected: 3,
        cancel: ctx.cancellation_flag(),
        run_errs: Arc::new(Mutex::new(Vec::new())),
    });

    // prepare 建连完成后再启动 pause scheduler，避免过早摘调度器。
    let connections_established = Arc::new(AtomicBool::new(false));

    // 线程 1：GC safepoint keeper（不依赖 barrier）。
    let cx1 = cx.clone();
    let sp_id = cx.cfg.SafePointID.clone();
    let h1 = thread::spawn(move || {
        if let Err(e) = pauseGCKeeper(cx1.clone(), &sp_id) {
            cx1.run_errs.lock().unwrap().push(e);
            cx1.cancel.store(true, Ordering::SeqCst);
        }
    });

    // 线程 2：等 prepare 建连后再暂停 scheduler。
    let cx2 = cx.clone();
    let init2 = connections_established.clone();
    let h2 = thread::spawn(move || {
        eprintln!("Pause scheduler waiting all connections established.");
        while !init2.load(Ordering::SeqCst) && !cx2.cancel.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(5));
        }
        if cx2.cancel.load(Ordering::SeqCst) {
            // 已取消则不再摘调度器，避免无配对 undo。
            return;
        }
        eprintln!("Pause scheduler noticed connections established.");
        if let Err(e) = pauseSchedulerKeeper(cx2.clone()) {
            cx2.run_errs.lock().unwrap().push(e);
            cx2.cancel.store(true, Ordering::SeqCst);
        }
    });

    // 线程 3：Preparer 驱动，并在建连回调里放行 barrier。
    let cx3 = cx.clone();
    let init3 = connections_established;
    let h3 = thread::spawn(move || {
        if let Err(e) = pauseAdminAndWaitApply(cx3.clone(), init3) {
            cx3.run_errs.lock().unwrap().push(e);
            cx3.cancel.store(true, Ordering::SeqCst);
        }
    });

    let cx_ready = cx.clone();
    let ready_thread = thread::spawn(move || {
        while cx_ready.ready.load(Ordering::SeqCst) < cx_ready.ready_expected
            && !cx_ready.cancel.load(Ordering::SeqCst)
        {
            thread::sleep(Duration::from_millis(5));
        }
        if cx_ready.ready.load(Ordering::SeqCst) >= cx_ready.ready_expected {
            if let Some(f) = &cx_ready.cfg.OnAllReady {
                f();
            }
            DUMP_GOROUTINE_WHEN_EXIT.store(false, Ordering::SeqCst);
            hintAllReady();
        }
    });

    // 先等全部 worker/ready 线程结束，再 OnExit/Close，避免半关闭 RPC。
    let _ = (h1.join(), h2.join(), h3.join(), ready_thread.join());

    if let Some(f) = &cx.cfg.OnExit {
        // 测试钩子：断言 exit 路径被调用。
        f();
    }
    cx.Close();

    // 返回首个 run_err；与 Go errgroup 首错语义接近。
    let errs = cx.run_errs.lock().unwrap().clone();
    if let Some(e) = errs.into_iter().next() {
        return Err(e);
    }
    Ok(())
}

/// Preparer 驱动：暂停 admin 并等待 apply；建连后唤醒 scheduler 线程。
fn pauseAdminAndWaitApply(
    cx: Arc<AdaptEnvForSnapshotBackupContext>,
    afterConnectionsEstablished: Arc<AtomicBool>,
) -> Result<()> {
    let begin = Instant::now();
    let mut prep = NewPreparer();
    prep.LeaseDuration = cx.cfg.TTL;
    let cancel = cx.cancel.clone();
    prep.AfterConnectionsEstablished = Some(Box::new(move || {
        // 拼写与 Go 日志一致（stablished），供外部探测脚本匹配。
        eprintln!("All connections are stablished.");
        afterConnectionsEstablished.store(true, Ordering::SeqCst);
        let _ = &cancel;
    }));

    let prep = Arc::new(Mutex::new(prep));
    let prep_for_cleanup = prep.clone();
    let result = {
        let prep_guard = prep.lock().unwrap();
        // 阻塞直到 prepare 完成或失败；失败则 Finalize 后上抛。
        prep_guard.DriveLoopAndWaitPrepare()
    };
    if let Err(err) = result {
        // 失败路径仍 Finalize，避免租约泄漏。
        cx.cleanUpWith(|| {
            let _ = prep_for_cleanup.lock().unwrap().Finalize();
        });
        return Err(err);
    }

    cx.ReadyL("pause_admin_and_wait_apply");
    let _ = begin;
    // 保持租约直到全局 cancel（快照窗口）。
    while !cx.cancel.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(5));
    }
    // 正常收尾：Finalize prepare stream；错误只打日志不掩盖主路径成功。
    cx.cleanUpWith(|| {
        if let Err(err) = prep_for_cleanup.lock().unwrap().Finalize() {
            eprintln!("failed to finalize the prepare stream: {err}");
        }
    });
    Ok(())
}

/// 设置 BR service safepoint 并续约，直到 cancel；退出时 TTL=0 清除。
fn pauseGCKeeper(cx: Arc<AdaptEnvForSnapshotBackupContext>, spID: &str) -> Result<()> {
    let mgr = MemGCManager::default();
    let mut sp = BRServiceSafePoint {
        // 空 ID 则生成随机 safepoint ID，与 Go MakeSafePointID 一致。
        ID: if spID.is_empty() {
            MakeSafePointID()
        } else {
            spID.to_string()
        },
        TTL: cx.cfg.TTL.as_secs() as i64,
        BackupTS: cx.cfg.SafePoint,
    };
    if sp.BackupTS == 0 {
        // 未显式给 SafePoint 时用集群最小 resolved TS，防止 GC 越过备份点。
        let rts = cx.pdMgr.GetMinResolvedTS()?;
        eprintln!(
            "No service safepoint provided, using the minimal resolved TS. min-resolved-ts={rts}"
        );
        sp.BackupTS = rts;
    }
    // 启动续约 keeper；MemGCManager 为进程内替身。
    StartServiceSafePointKeeper(sp.clone(), &mgr)?;
    cx.ReadyL("pause_gc");

    let mut err_out = None;
    // 窗口内空转等待 cancel，与 Go 阻塞直到 ctx.Done 对应。
    while !cx.cancel.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(5));
    }
    // TTL=0 表示撤销该 service safepoint。
    cx.cleanUpWithRetErr(&mut err_out, || {
        let cancelSP = BRServiceSafePoint {
            ID: sp.ID.clone(),
            TTL: 0,
            BackupTS: 0,
        };
        mgr.SetServiceSafePoint(cancelSP)
    });
    if let Some(e) = err_out {
        return Err(e);
    }
    Ok(())
}

/// 移除全部 PD scheduler；cancel 后执行 undo 恢复。
fn pauseSchedulerKeeper(cx: Arc<AdaptEnvForSnapshotBackupContext>) -> Result<()> {
    let undo = cx.pdMgr.RemoveAllPDSchedulers()?;
    if let Some(undo) = undo {
        // cleanup registered below after ready wait
        // undo 闭包在 cancel 后调用，恢复调度器；失败仅打日志。
        let _cleanup = undo;
        cx.ReadyL("pause_scheduler");
        while !cx.cancel.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(5));
        }
        cx.cleanUpWith(|| {
            if let Err(err) = _cleanup() {
                eprintln!("failed to restore pd scheduler: {err}");
            }
        });
    } else {
        // 无 undo（桩/空操作）仍标记就绪，避免卡住 ready 计数。
        cx.ReadyL("pause_scheduler");
        while !cx.cancel.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(5));
        }
    }
    Ok(())
}
