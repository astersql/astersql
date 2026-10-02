// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Store-level backup send / receive / timeout / split logic matching `store.go`.
//!
//! 本模块对齐 Go `br/pkg/backup/store.go`，负责**单 store** 维度的备份收发：
//! - 将大请求按并发度切分为多个子请求（`SplitBackupReqRanges`）；
//! - 对每个子请求建立带超时看门狗的 gRPC Backup 流（`StartTimeoutRecv`/`doSendBackup`）；
//! - 把响应连同 storeID 投递到上层通道（`ResponseAndStore`）；
//! - 异步观察 PD store 拓扑变化，驱动重试策略（`ObserveStoreChangesAsync`）。
//!
//! 数据流：`BackupSender::SendAsync`（实现在 client）→ `startBackup` →
//! 工作池并发 `doSendBackup` → `respCh`；超时未 Refresh 则 cancel cause。
//! 不在此文件建立 PD/TiKV 连接，客户端由调用方注入。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::limit::ResourceConcurrentLimiter;
use crate::stubs::backuppb::{self, BackupClient, BackupRequest, BackupResponse};
use crate::stubs::failpoint;
use crate::stubs::storewatch;
use crate::stubs::utils::{self, WithRetry};
use crate::stubs::{Context, Error, PdClient, Result, WorkerPool, wait_jobs};

/// BackupRetryPolicy mirrors Go `BackupRetryPolicy`.
///
/// 重试策略载荷：`One` 指定单个 store 重试；`All=true` 表示全量重发。
/// 由 store 变更观察或发送失败路径写入 `StateNotifier`，供主循环消费。
#[derive(Clone, Debug, Default)]
pub struct BackupRetryPolicy {
    /// 需要重试的单个 store ID；与 `All` 互斥使用（Go 同字段语义）。
    pub One: u64,
    /// 为 true 时忽略 `One`，对所有 store 触发重试（如 reboot/disconnect）。
    pub All: bool,
}

/// BackupSender mirrors Go `BackupSender`.
///
/// 抽象“向某 store 异步发起一轮备份”的入口；生产实现为 `MainBackupSender`。
/// `respCh` 收到 `None` 表示该 store 本轮发送结束（关闭标记）。
pub trait BackupSender: Send + Sync {
    /// 异步向指定 store 发送一轮备份请求；实现方负责线程/错误与关闭标记。
    fn SendAsync(
        &self,
        ctx: Context,
        // 当前备份轮次，失败重试通知时用于日志/诊断。
        round: u64,
        // 目标 TiKV store。
        storeID: u64,
        // 跨 store 共享的并发/内存类限流器。
        limiter: Arc<ResourceConcurrentLimiter>,
        request: BackupRequest,
        // 期望的分片并发度，传入 `SplitBackupReqRanges`。
        concurrency: u32,
        cli: Arc<dyn BackupClient>,
        // 响应通道；`None` 为本 store 本轮结束哨兵。
        respCh: Sender<Option<ResponseAndStore>>,
        // 发送失败等场景写入重试策略。
        StateNotifier: Sender<BackupRetryPolicy>,
    );
}

/// ResponseAndStore mirrors Go `ResponseAndStore`.
///
/// 将 Backup 流响应与来源 store 绑定，便于主循环按 store 聚合进度/错误。
#[derive(Clone, Debug)]
pub struct ResponseAndStore {
    /// 原始 Backup RPC 响应（可能含 Error/RegionError）。
    pub Resp: BackupResponse,
    /// 响应对应的 store，供主循环路由。
    pub StoreID: u64,
}

impl ResponseAndStore {
    /// 取出响应引用；对齐 Go 访问器，避免直接暴露字段语义漂移。
    pub fn GetResponse(&self) -> &BackupResponse {
        &self.Resp
    }
    /// 取出产生该响应的 TiKV store ID。
    pub fn GetStoreID(&self) -> u64 {
        self.StoreID
    }
}

/// timeoutRecv cancels when Refresh is not called within `timeout`.
///
/// 看门狗：若在 `timeout` 内未收到 `Refresh`（通常由成功收到一包响应触发），
/// 则以 “receive a backup response timeout” 为 cause 取消派生 context。
/// Rust 用 `AtomicBool`+同步通道模拟 Go 的 `parentCtx`/`refresh` channel。
pub struct timeoutRecv {
    storeID: u64,
    /// 父 context 或本端 Stop 后置位，驱动循环退出。
    parent_done: Arc<AtomicBool>,
    cancel: crate::stubs::CancelCauseFunc,
    /// 容量 1 的刷新信号；`Stop` 时 take 掉以关闭接收端。
    refresh_tx: Mutex<Option<SyncSender<()>>>,
    join: Mutex<Option<JoinHandle<()>>>,
    /// 幂等 Stop：重复调用直接返回。
    stopped: AtomicBool,
}

impl timeoutRecv {
    /// 刷新超时计时；父已结束或通道已关闭时静默忽略（对齐 Go select 行为）。
    pub fn Refresh(&self) {
        if self.parent_done.load(Ordering::SeqCst) {
            return;
        }
        if let Some(tx) = self.refresh_tx.lock().unwrap().as_ref() {
            let _ = tx.try_send(());
        }
    }

    /// 关闭刷新通道、等待循环线程、再 cancel；保证与 Go `close+Wait+cancel` 顺序一致。
    pub fn Stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        // Drop the sender to close the refresh channel (mirrors Go `close(trecv.refresh)`).
        // 丢弃 sender 等价于关闭 channel，循环侧收到 Disconnected 后退出。
        drop(self.refresh_tx.lock().unwrap().take());
        self.parent_done.store(true, Ordering::SeqCst);
        if let Some(h) = self.join.lock().unwrap().take() {
            let _ = h.join();
        }
        self.cancel.cancel(None);
    }
}

/// TimeoutOneResponse mirrors Go `TimeoutOneResponse` (1 hour).
///
/// 单次响应等待上限：生产默认 1 小时，避免 TiKV 卡住时永久挂起。
pub static TimeoutOneResponse: Duration = Duration::from_secs(3600);

/// Override used by parity tests (None => TimeoutOneResponse).
///
/// 仅测试注入短超时；生产路径保持 `None`，读取时回落默认值。
static TIMEOUT_OVERRIDE: Mutex<Option<Duration>> = Mutex::new(None);

/// 测试钩子：覆盖单响应超时并返回旧值，供测试清理时精确恢复。
pub fn set_timeout_one_response_for_test(d: Option<Duration>) -> Option<Duration> {
    std::mem::replace(&mut *TIMEOUT_OVERRIDE.lock().unwrap(), d)
}

/// 解析当前生效超时：优先测试覆盖，否则 `TimeoutOneResponse`。
fn effective_timeout() -> Duration {
    TIMEOUT_OVERRIDE
        .lock()
        .unwrap()
        .unwrap_or(TimeoutOneResponse)
}

/// StartTimeoutRecv mirrors Go `StartTimeoutRecv`.
///
/// 派生可取消子 context，并启动超时循环线程。
/// 另起线程轮询父 context Done，将取消传播到 `parent_done`（Go 直接读 parentCtx）。
pub fn StartTimeoutRecv(
    ctx: &Context,
    timeout: Duration,
    storeID: u64,
) -> (Context, Arc<timeoutRecv>) {
    let (cctx, cancel) = Context::WithCancelCause(ctx);
    let parent_done = Arc::new(AtomicBool::new(false));
    let parent_flag = parent_done.clone();
    let parent_ctx = ctx.clone();
    // Watch parent cancellation.
    // 父取消时尽快置位，避免超时循环在父已结束后仍空转。
    {
        let flag = parent_flag.clone();
        let p = parent_ctx.clone();
        thread::spawn(move || {
            while !p.Done() && !flag.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            flag.store(true, Ordering::SeqCst);
        });
    }

    let (tx, rx) = mpsc::sync_channel::<()>(1);
    let cancel_for_loop = cancel.clone();
    let parent_for_loop = parent_done.clone();
    let join = thread::spawn(move || {
        loop_timeout(rx, timeout, storeID, parent_for_loop, cancel_for_loop);
    });

    let trecv = Arc::new(timeoutRecv {
        storeID,
        parent_done,
        cancel,
        refresh_tx: Mutex::new(Some(tx)),
        join: Mutex::new(Some(join)),
        stopped: AtomicBool::new(false),
    });
    (cctx, trecv)
}

/// 超时循环：收到 Refresh 则重置等待；超时则带 cause 取消；通道断开则退出。
/// `storeID` 在 Go 中用于日志字段，此处保留参数以对齐签名。
fn loop_timeout(
    rx: Receiver<()>,
    timeout: Duration,
    storeID: u64,
    parent_done: Arc<AtomicBool>,
    cancel: crate::stubs::CancelCauseFunc,
) {
    let _ = storeID;
    loop {
        if parent_done.load(Ordering::SeqCst) {
            return;
        }
        match rx.recv_timeout(timeout) {
            Ok(()) => {
                // refreshed — 成功收到心跳，下一轮重新计时
            }
            Err(RecvTimeoutError::Timeout) => {
                // 与 Go 错误文案一致，供上层识别超时取消原因。
                cancel.cancel(Some(Error::new("receive a backup response timeout")));
                return;
            }
            Err(RecvTimeoutError::Disconnected) => {
                // Stop 关闭通道后的正常退出路径。
                return;
            }
        }
    }
}

/// doSendBackup mirrors Go `doSendBackup`.
///
/// 对单个 `BackupRequest` 建立流、逐包 `Recv` 并回调 `respFn`。
/// 在调用 Backup 前后 Acquire/Release limiter，令牌数 = SubRanges+1（含主 range）。
/// failpoint 可注入启动信号、可重试/不可重试错误，仅测试路径生效。
pub fn doSendBackup(
    ctx: &Context,
    client: &dyn BackupClient,
    limiter: &ResourceConcurrentLimiter,
    req: BackupRequest,
    mut respFn: impl FnMut(&BackupResponse) -> Result<()>,
) -> Result<()> {
    // failpoint：备份开始时写信号文件，并可选 sleep 便于竞态测试。
    if let Some(sig) = failpoint::take_hint_backup_start() {
        let _ = std::fs::File::create(&sig);
        if !crate::stubs::should_skip_round_sleep() {
            thread::sleep(Duration::from_secs(3));
        }
    }

    let reqStartKey = req.StartKey.clone();
    let reqEndKey = req.EndKey.clone();
    // Go：range 计数含主区间，故 +1；限流防止过多并发 Backup RPC。
    let reqRangeSize = req.SubRanges.len() + 1;
    limiter.Acquire(reqRangeSize as isize);
    let mut stream_result = client.Backup(ctx, &req);
    limiter.Release(reqRangeSize as isize);

    // 注入 gRPC 风格可重试错误（Unavailable/Internal）。
    if let Some(kind) = failpoint::take_reset_retryable() {
        match kind.as_str() {
            "Unavailable" => {
                stream_result = Err(Error::new("Unavailable error"));
            }
            "Internal" => {
                stream_result = Err(Error::new("Internal error"));
            }
            _ => {}
        }
    }
    // 注入不可重试错误，验证上层不会无意义重试。
    if failpoint::take_reset_not_retryable() {
        stream_result = Err(Error::new(
            "Your server was haunted hence doesn't work, meow :3",
        ));
    }

    let mut bCli = match stream_result {
        Ok(s) => s,
        Err(e) => return Err(e),
    };
    // 读循环：ctx 取消立即返回；Recv None 表示流结束。
    let close_result = (|| -> Result<()> {
        loop {
            if ctx.Done() {
                return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
            }
            match bCli.Recv()? {
                None => {
                    // 保留起止键引用与 Go 日志字段对齐（当前无日志桩）。
                    let _ = (&reqStartKey, &reqEndKey);
                    return Ok(());
                }
                Some(resp) => {
                    respFn(&resp)?;
                }
            }
        }
    })();
    // 无论读成功与否都尝试 CloseSend，对齐 Go defer 关闭语义。
    let _ = bCli.CloseSend();
    close_result
}

/// startBackup mirrors Go `startBackup`.
///
/// 单 store 备份编排：切分请求 → 启超时看门狗 → 工作池并发发送 → 等待全部 job。
/// 每个响应成功入队后 Refresh 看门狗；全部结束后 Stop，再返回聚合错误。
pub fn startBackup(
    pctx: &Context,
    storeID: u64,
    limiter: Arc<ResourceConcurrentLimiter>,
    backupReq: BackupRequest,
    backupCli: Arc<dyn BackupClient>,
    concurrency: u32,
    respCh: Sender<Option<ResponseAndStore>>,
) -> Result<()> {
    if pctx.Done() {
        return Err(pctx.Err().unwrap_or_else(|| Error::new("context canceled")));
    }

    // 按并发度切分 SubRanges，每个分片独立重试退避。
    let reqs = SplitBackupReqRanges(backupReq, concurrency as isize);
    let timeout = effective_timeout();
    let (ctx, timerecv) = StartTimeoutRecv(pctx, timeout, storeID);
    let pool = WorkerPool::new(concurrency, "store_backup");
    let mut jobs = Vec::new();
    let ectx = ctx.clone();

    for (i, req) in reqs.into_iter().enumerate() {
        let bkReq = req;
        let reqIndex = i;
        let ectx2 = ectx.clone();
        let cli = backupCli.clone();
        let limiter2 = limiter.clone();
        let respCh2 = respCh.clone();
        let timerecv2 = timerecv.clone();
        let storeID2 = storeID;
        pool.ApplyOnErrorGroup(&mut jobs, move || {
            let mut retry = -1isize;
            // WithRetry + BackupSST 退避策略，对齐 Go utils.WithRetry。
            WithRetry(
                &ectx2,
                || {
                    retry += 1;
                    let _ = (retry, reqIndex, storeID2);
                    doSendBackup(
                        &ectx2,
                        cli.as_ref(),
                        limiter2.as_ref(),
                        bkReq.clone(),
                        |resp| {
                            let mut resp = resp.clone();
                            // 下列 failpoint 在响应路径注入各类错误，覆盖超时/存储/读写/Region。
                            if let Some(msg) = failpoint::take_backup_timeout_error() {
                                resp.Error = Some(backuppb::Error {
                                    Msg: msg,
                                    Detail: backuppb::ErrorDetail::None,
                                });
                            }
                            if let Some(msg) = failpoint::take_backup_storage_error() {
                                resp.Error = Some(backuppb::Error {
                                    Msg: msg,
                                    Detail: backuppb::ErrorDetail::None,
                                });
                            }
                            if let Some(msg) = failpoint::take_tikv_rw_error() {
                                resp.Error = Some(backuppb::Error {
                                    Msg: msg,
                                    Detail: backuppb::ErrorDetail::None,
                                });
                            }
                            if let Some(msg) = failpoint::take_tikv_region_error() {
                                resp.Error = Some(backuppb::Error {
                                    Msg: String::new(),
                                    Detail: backuppb::ErrorDetail::RegionError {
                                        RegionError: crate::stubs::errorpb::Error { Message: msg },
                                    },
                                });
                            }
                            if ectx2.Done() {
                                return Err(ectx2
                                    .Err()
                                    .unwrap_or_else(|| Error::new("context canceled")));
                            }
                            // 非空 Some 推送；None 关闭标记由 SendAsync 在 store 结束时发送。
                            respCh2
                                .send(Some(ResponseAndStore {
                                    Resp: resp,
                                    StoreID: storeID2,
                                }))
                                .map_err(|_| Error::new("resp channel closed"))?;
                            // 每成功投递一包即刷新，防止慢流被误判超时。
                            timerecv2.Refresh();
                            Ok(())
                        },
                    )
                },
                utils::NewBackupSSTBackoffStrategy(),
            )
        });
    }
    // 先等全部分片结束，再 Stop 看门狗，避免 Refresh 与 Stop 竞态。
    let wait_err = wait_jobs(jobs);
    timerecv.Stop();
    wait_err
}

/// ObserveStoreChangesAsync mirrors Go `ObserveStoreChangesAsync`.
///
/// 后台线程通过 `storewatch` 轮询 PD：reboot/disconnect → `All` 重试；
/// 新注册 store → 按 ID 发 `One` 重试。tick 默认 30s，failpoint 可缩至 100ms。
pub fn ObserveStoreChangesAsync(
    ctx: Context,
    stateNotifier: Sender<BackupRetryPolicy>,
    pdCli: Arc<dyn PdClient>,
) {
    thread::spawn(move || {
        let sendAll = Arc::new(AtomicBool::new(false));
        let newJoinStoresMap: Arc<Mutex<HashMap<u64, ()>>> = Arc::new(Mutex::new(HashMap::new()));

        let sendAll2 = sendAll.clone();
        let sendAll3 = sendAll.clone();
        let newMap = newJoinStoresMap.clone();
        // 回调在 Step 期间累积标志；一轮 tick 结束后统一通知，避免风暴。
        let cb = storewatch::MakeCallback(
            storewatch::WithOnReboot(move |_s| {
                sendAll2.store(true, Ordering::SeqCst);
            }),
            storewatch::WithOnDisconnect(move |_s| {
                sendAll3.store(true, Ordering::SeqCst);
            }),
            storewatch::WithOnNewStoreRegistered(move |s| {
                newMap.lock().unwrap().insert(s.Id, ());
            }),
        );

        let notifyFn = |ctx: &Context, sendPolicy: BackupRetryPolicy| {
            if ctx.Done() {
                return;
            }
            let _ = stateNotifier.send(sendPolicy);
        };

        let watcher = storewatch::New(pdCli, cb);
        if let Err(_e) = watcher.Step(&ctx) {
            // ignore beginning watch failure (Go logs warn)
            // 初始 Step 失败仅告警，不中断观察循环（Go 同策略）。
        }

        let mut tickInterval = Duration::from_secs(30);
        if failpoint::take_backup_store_change_tick() {
            // 测试加速：缩短拓扑轮询间隔。
            tickInterval = Duration::from_millis(100);
        }

        while !ctx.Done() {
            thread::sleep(tickInterval);
            if ctx.Done() {
                return;
            }
            // 每轮清空累积状态，只反映本 tick 内的变化。
            sendAll.store(false, Ordering::SeqCst);
            newJoinStoresMap.lock().unwrap().clear();
            if let Err(_e) = watcher.Step(&ctx) {
                // ignore — Step 瞬时失败不退出，下轮再试。
            }
            if sendAll.load(Ordering::SeqCst) {
                notifyFn(&ctx, BackupRetryPolicy { One: 0, All: true });
            } else {
                let ids: Vec<u64> = newJoinStoresMap.lock().unwrap().keys().copied().collect();
                for storeID in ids {
                    notifyFn(
                        &ctx,
                        BackupRetryPolicy {
                            One: storeID,
                            All: false,
                        },
                    );
                }
            }
        }
    });
}

/// SplitBackupReqRanges mirrors Go `SplitBackupReqRanges`.
///
/// 将 `SubRanges` 尽量均分到 `count` 个请求：前 `overCount` 个多分 1 段。
/// 无 SubRanges 或 count≤1 时原样返回单请求；某分片长度为 0 则提前结束。
/// 主 StartKey/EndKey 随 clone 保留，仅替换 SubRanges 切片。
pub fn SplitBackupReqRanges(req: BackupRequest, count: isize) -> Vec<BackupRequest> {
    let rangeCount = req.SubRanges.len() as isize;
    if rangeCount == 0 {
        return vec![req];
    }
    if count <= 1 {
        return vec![req];
    }
    let count = count as usize;
    let rangeCount = rangeCount as usize;
    let splitStep = rangeCount / count;
    // 余数分配给前几个分片，保证覆盖全部 range 且尽量均衡。
    let overCount = rangeCount - count * splitStep;
    let mut splitRequests = Vec::with_capacity(count);
    let mut start = 0usize;
    for i in 0..count {
        let mut nextStart = start + splitStep;
        if i < overCount {
            nextStart += 1;
        } else if nextStart == start {
            // 后续分片已无剩余 range，停止产生空请求。
            break;
        }
        let mut splitReq = req.clone();
        splitReq.SubRanges = req.SubRanges[start..nextStart].to_vec();
        splitRequests.push(splitReq);
        start = nextStart;
    }
    splitRequests
}

// silence unused import warning for TryRecvError in some builds
// 保留 TryRecvError 引用以免部分 feature 组合下未使用告警。
#[allow(dead_code)]
fn _unused_try_recv(_: TryRecvError) {}
