// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
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

//! Import mode switcher matching `import_mode_switcher.go`.
//!
//! 模块职责：在离线恢复前后把 TiKV 切入/切出 import mode，并配合摘除/恢复 PD
//! scheduler；后台按间隔刷新 Import，防止 TiKV 自动退回 Normal。
//! 对应 Go `import_mode_switcher.go`；TiFlash store 经 SkipTiFlash 过滤不切换。
//! 约束：SwitchMode 经 ImportSstSwitcher 抽象；GrpcImportSstSwitcher 提供真实 gRPC 传输。

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::stubs::{
    self, CancelFunc, ClusterConfig, ConnMgr, Context, Error, ErrorGroup, ImportSstSwitcher, Key,
    NewWorkerPool, PdClient, Result, UndoFunc, WaitGroup, import_sstpb, log, metapb, nop_undo,
};

/// Store label skip filter matching Go `util.SkipTiFlash`.
/// 返回 true 表示保留该 store；engine=tiflash（忽略大小写）被过滤掉。
pub fn SkipTiFlash(store: &metapb::Store) -> bool {
    !store
        .Labels
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("engine") && v.eq_ignore_ascii_case("tiflash"))
}

/// 从 PD 拉全量 store 再按 keep 过滤，通常传入 SkipTiFlash。
pub fn GetAllTiKVStores(
    ctx: &Context,
    pd: &dyn PdClient,
    keep: impl Fn(&metapb::Store) -> bool,
) -> Result<Vec<metapb::Store>> {
    let stores = pd.GetAllStores(ctx)?;
    // keep 通常为 SkipTiFlash，结果只含应切换的 TiKV store。
    Ok(stores.into_iter().filter(|s| keep(s)).collect())
}

/// 持有 PD/Switcher 与刷新间隔；cancel+WaitGroup 管理后台刷新协程生命周期。
pub struct ImportModeSwitcher {
    pd_client: Arc<dyn PdClient>,
    switcher: Arc<dyn ImportSstSwitcher>,
    switch_mode_interval: Duration,
    mu: Mutex<()>,
    cancel: Option<CancelFunc>,
    refresh_wake: Option<Sender<()>>,
    wg: Arc<WaitGroup>,
}

/// 构造切换器；初始处于 Normal（cancel=None），需显式 GoSwitchToImportMode。
pub fn NewImportModeSwitcher(
    pd_client: Arc<dyn PdClient>,
    switch_mode_interval: Duration,
    switcher: Arc<dyn ImportSstSwitcher>,
) -> ImportModeSwitcher {
    ImportModeSwitcher {
        pd_client,
        switcher,
        switch_mode_interval,
        mu: Mutex::new(()),
        cancel: None,
        refresh_wake: None,
        wg: Arc::new(WaitGroup::new()),
    }
}

impl ImportModeSwitcher {
    /// End the operation's refresh worker without restoring TiKV/PD state.
    /// Used when snapshot checkpoint retry intentionally retains paused state.
    pub fn StopRefreshing(&mut self) {
        let _guard = self.mu.lock().unwrap();
        if let Some(cancel) = self.cancel.take() {
            cancel.call();
        }
        if let Some(wake) = self.refresh_wake.take() {
            let _ = wake.send(());
        }
        self.wg.Wait();
    }
    /// SwitchToNormalMode stops the import-mode refresh goroutine and switches TiKV to normal.
    /// 先停刷新协程并 Wait，再并发 SwitchMode(Normal)；已是 Normal 则直接成功。
    pub fn SwitchToNormalMode(&mut self, ctx: &Context) -> Result<()> {
        let _guard = self.mu.lock().unwrap();

        // cancel 为空表示未进入 import 刷新循环。
        if self.cancel.is_none() {
            log::Info("TiKV is already in normal mode");
            return Ok(());
        }
        log::Info("Stopping the import mode goroutine");
        // take cancel 后触发 Done，Wait 等后台线程退出再切 Normal。
        if let Some(cancel) = self.cancel.take() {
            cancel.call();
        }
        if let Some(wake) = self.refresh_wake.take() {
            let _ = wake.send(());
        }
        self.wg.Wait();
        self.switchTiKVMode(ctx, import_sstpb::SwitchMode::Normal)
    }

    /// 对所有非 TiFlash store 并发 SwitchMode；任一失败经 ErrorGroup 汇总。
    pub fn switchTiKVMode(&self, ctx: &Context, mode: import_sstpb::SwitchMode) -> Result<()> {
        let stores =
            GetAllTiKVStores(ctx, self.pd_client.as_ref(), SkipTiFlash).map_err(Error::Trace)?;
        // 池大小=store 数，尽量一轮并发切完。
        let worker_pool = NewWorkerPool(stores.len() as u64, "switch import mode");
        let (eg, ectx) = ErrorGroup::with_context(ctx);
        for store in stores {
            // 已有任务失败则不再提交新任务，尽快收敛。
            if let Some(err) = ectx.Err() {
                return Err(Error::Trace(err));
            }
            // 按 Address 拨号切换，不依赖 StoreId。
            let addr = store.GetAddress().to_string();
            let switcher = self.switcher.clone();
            let job_ctx = ectx.clone();
            worker_pool.ApplyOnErrorGroup(&eg, move || {
                // Go dials gRPC with 5s timeout then SwitchMode; local trait stands in for that boundary.
                // 超时 Context 占位对齐 Go 5s dial；实际切换走 switcher trait。
                let (_gctx, _cancel) = Context::WithTimeout(&job_ctx, Duration::from_secs(5));
                switcher
                    .SwitchMode(&job_ctx, &addr, mode)
                    .map_err(Error::Trace)
            });
        }
        eg.Wait().map_err(Error::Trace)
    }

    /// GoSwitchToImportMode switches immediately to import mode and starts periodic refresh.
    /// 立即切 Import 并启动后台按 interval 刷新；已在 Import 则幂等返回。
    pub fn GoSwitchToImportMode(&mut self, ctx: &Context) -> Result<()> {
        let _guard = self.mu.lock().unwrap();

        // 已有 cancel 说明刷新协程在跑，避免重复 spawn。
        if self.cancel.is_some() {
            log::Info("TiKV is already in import mode");
            return Ok(());
        }

        let (bg_ctx, cancel) = Context::WithCancel(ctx);
        self.cancel = Some(cancel);
        let (refresh_wake, refresh_wait) = mpsc::channel();
        self.refresh_wake = Some(refresh_wake);

        log::Info("switch to import mode at beginning");
        if let Err(err) = self.switchTiKVMode(&bg_ctx, import_sstpb::SwitchMode::Import) {
            log::Warn("switch to import mode failed");
            // Match Go: leave cancel set even when the initial switch fails.
            // 初始切换失败仍保留 cancel，与 Go 一致，防止状态机分叉。
            return Err(Error::Trace(err));
        }

        self.wg.Add(1);
        let interval = self.switch_mode_interval;
        let pd = self.pd_client.clone();
        let switcher = self.switcher.clone();
        let wg = self.wg.clone();
        // 后台线程持有 switcher 字段克隆，循环 sleep+SwitchMode(Import)。
        thread::spawn(move || {
            let local = ImportModeSwitcher {
                pd_client: pd,
                switcher,
                switch_mode_interval: interval,
                mu: Mutex::new(()),
                cancel: None,
                refresh_wake: None,
                wg: wg.clone(),
            };
            loop {
                if bg_ctx.Done() {
                    log::Info("stop automatic switch to import mode when context done");
                    break;
                }
                match refresh_wait.recv_timeout(interval) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                        log::Info("stop automatic switch to import mode when context done");
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
                log::Info("switch to import mode");
                // 刷新失败只打日志，不退出循环，对齐 Go 容忍瞬时错误。
                if let Err(_err) = local.switchTiKVMode(&bg_ctx, import_sstpb::SwitchMode::Import) {
                    log::Warn("switch to import mode failed");
                }
            }
            // 退出循环后 Done，供 SwitchToNormalMode 的 Wait 返回。
            wg.Done();
        });
        Ok(())
    }
}

/// RestorePreWork switches to import mode and removes PD schedulers if needed.
/// 在线恢复直接返回空 undo；离线时可切 Import 并 RemoveSchedulersWithConfig。
pub fn RestorePreWork(
    ctx: &Context,
    mgr: &dyn ConnMgr,
    switcher: &mut ImportModeSwitcher,
    is_online: bool,
    switch_to_import: bool,
) -> Result<(UndoFunc, Option<ClusterConfig>)> {
    // 在线模式不改集群调度/import，避免影响业务流量。
    if is_online {
        return Ok((nop_undo(), None));
    }
    // switch_to_import=false 时只摘 scheduler，用于部分恢复场景。
    if switch_to_import {
        switcher.GoSwitchToImportMode(ctx)?;
    }
    mgr.RemoveSchedulersWithConfig(ctx)
}

/// FineGrainedRestorePreWork pauses schedulers only on the given key ranges.
/// 细粒度：只在指定 key range 上挂 pause rule，并返回可撤销的 UndoFunc。
pub fn FineGrainedRestorePreWork(
    ctx: &Context,
    mgr: &dyn ConnMgr,
    switcher: &mut ImportModeSwitcher,
    key_range: &[[Key; 2]],
    switch_to_import: bool,
) -> Result<(UndoFunc, ClusterConfig)> {
    // 细粒度路径同样可选切入 Import，再按 range 暂停调度。
    if switch_to_import {
        log::Info("switch to import mode for offline restore");
        switcher.GoSwitchToImportMode(ctx)?;
    }
    let origin_cfg = mgr.GetOriginPDConfig(ctx)?;
    // 记录 rule_id/wait_pause，供 MakeFineGrainedUndoFunction 精确回滚。
    let (rule_id, wait_pause) = mgr.RemoveSchedulersOnRegion(ctx, key_range)?;
    // 在原始配置副本上写入 RuleID，undo 时按新配置回滚 pause。
    let mut new_cfg = origin_cfg.clone();
    new_cfg.RuleID = rule_id;
    let undo = mgr.MakeFineGrainedUndoFunction(new_cfg, wait_pause);
    Ok((undo, origin_cfg))
}

/// RestorePostWork switches back to normal mode and restores PD schedulers.
/// 上下文若已取消则换 Background 继续收尾，保证尽量切回 Normal 并执行 undo。
pub fn RestorePostWork(
    mut ctx: Context,
    switcher: &mut ImportModeSwitcher,
    restore_schedulers: UndoFunc,
    is_online: bool,
) {
    // 取消后的 ctx 无法完成 RPC，换成 Background 做尽力而为的清理。
    if ctx.Err().is_some() {
        log::Warn("context canceled, try shutdown");
        ctx = Context::Background();
    }
    // 切 Normal 失败只告警，仍继续尝试恢复 scheduler。
    if !is_online {
        if let Err(_err) = switcher.SwitchToNormalMode(&ctx) {
            log::Warn("fail to switch to normal mode");
        }
    }
    // undo 失败同样不 panic，避免掩盖主流程已完成的恢复结果。
    if let Err(_err) = restore_schedulers(&ctx) {
        log::Warn("failed to restore PD schedulers");
    }
}

/// ImportSST network boundary used by a live restore lifecycle. TLS credentials
/// are constructed by the caller from the same restore TLS configuration.
pub struct GrpcImportSstSwitcher {
    pub environment: Arc<grpcio::Environment>,
    pub credentials: Option<Arc<dyn Fn() -> grpcio::ChannelCredentials + Send + Sync>>,
}
impl ImportSstSwitcher for GrpcImportSstSwitcher {
    fn SwitchMode(&self, ctx: &Context, addr: &str, mode: import_sstpb::SwitchMode) -> Result<()> {
        if let Some(error) = ctx.Err() {
            return Err(error);
        }
        let builder = grpcio::ChannelBuilder::new(self.environment.clone())
            .max_reconnect_backoff(Duration::from_secs(3));
        let channel = match &self.credentials {
            Some(credentials) => builder.set_credentials(credentials()).connect(addr),
            None => builder.connect(addr),
        };
        // Register a connectivity watch on the completion queue; querying the
        // state alone does not drive this client's connection establishment.
        {
            let mut connected = Box::pin(channel.wait_for_connected(Duration::from_secs(5)));
            let mut poll_context = std::task::Context::from_waker(std::task::Waker::noop());
            loop {
                if let Some(error) = ctx.Err() {
                    return Err(error);
                }
                match std::future::Future::poll(connected.as_mut(), &mut poll_context) {
                    std::task::Poll::Ready(true) => break,
                    std::task::Poll::Ready(false) => {
                        return Err(Error::new(format!("dial ImportSST at {addr}: timeout")));
                    }
                    std::task::Poll::Pending => thread::sleep(Duration::from_millis(10)),
                }
            }
        }
        let client = kvproto::import_sstpb_grpc::ImportSstClient::new(channel);
        let mut request = kvproto::import_sstpb::SwitchModeRequest::default();
        request.set_mode(match mode {
            import_sstpb::SwitchMode::Normal => kvproto::import_sstpb::SwitchMode::Normal,
            import_sstpb::SwitchMode::Import => kvproto::import_sstpb::SwitchMode::Import,
        });
        // Go bounds dialing by five seconds, but the RPC lifetime is the parent
        // operation context. Poll the async receiver so cancellation reaches gRPC.
        let mut response = client
            .switch_mode_async(&request)
            .map_err(|error| Error::new(format!("switch TiKV mode at {addr}: {error}")))?;
        let mut poll_context = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let Some(error) = ctx.Err() {
                response.cancel();
                return Err(error);
            }
            match std::future::Future::poll(std::pin::Pin::new(&mut response), &mut poll_context) {
                std::task::Poll::Ready(result) => {
                    result.map_err(|error| {
                        Error::new(format!("switch TiKV mode at {addr}: {error}"))
                    })?;
                    break;
                }
                std::task::Poll::Pending => thread::sleep(Duration::from_millis(10)),
            }
        }
        Ok(())
    }
}
