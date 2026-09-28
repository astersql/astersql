// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Per-store prepare stream ported from `br/pkg/backup/prepare_snap/stream.go`.
//!
//! 每 store 一条 PrepareSnapshot 双向流，对齐 Go `stream.go`。
//! 职责：初始化 lease、后台收包/续约、把 WaitApplyDone 转成 event 交给 Preparer。
//! Finalize 停后台循环、发 Finish，并排空剩余响应。
//! `AsyncStreamBy` 把阻塞 Recv 泵到 channel，避免主线程卡在 gRPC。
//! UpdateLeaseResult 若 LastLeaseIsValid=false，上抛 leaseExpired。
//! client_loop 与 Finalize 共享收包端，停止顺序必须先 stop 再 join。
//! 本文件不直接依赖真实 gRPC；PrepareClient 可由测试桩替换。

// 后台线程 + 原子标志驱动续约循环；mpsc 承载收包与事件投递。
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// PrepareClient 是真实/测试共用的发送接收边界。
use crate::env::{Context, PrepareClient, brpb, metapb};
use crate::errors::{Error, Result, convertErr, leaseExpired, unsupported};

/// 事件类型别名，数值与 Go 侧常量一致。
pub type eventType = i32;

/// 不可恢复/杂项错误事件（含 lease 过期）。
pub const eventMiscErr: eventType = 0;
/// WaitApply 完成事件，携带 region 与可选错误。
pub const eventWaitApplyDone: eventType = 1;

/// 流向 Preparer 的统一事件载体。
/// region 仅在 WaitApplyDone 时有值。
pub struct event {
    /// 事件类型。
    pub ty: eventType,
    /// 目标 store id。
    pub storeID: u64,
    /// 可选错误；misc 路径必填。
    pub err: Option<Error>,
    /// WaitApply 对应的 region。
    pub region: Option<metapb::Region>,
}

/// 调试输出，便于日志对照 Go 事件字符串。
impl fmt::Display for event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Event(Type: {}, StoreID: {}, Error: {:?}, Region: {:?})",
            self.ty, self.storeID, self.err, self.region
        )
    }
}

/// Result envelope matching `utils.Result[T]`.
/// Result envelope matching `utils.Result[T]`.
/// 对齐 Go utils.Result：Err 与 Item 并存，错误时 Item 为 Default。
pub struct StreamResult<T> {
    /// 流错误；Some 表示生成器终止。
    pub Err: Option<Error>,
    /// 成功时的载荷。
    pub Item: T,
}

/// Streams items produced by `generator` from a background thread (Go `AsyncStreamBy`).
/// Streams items produced by `generator` from a background thread (Go `AsyncStreamBy`).
/// 后台循环调用 generator；首个 Err 发送后退出，接收端断开亦退出。
pub fn AsyncStreamBy<T, F>(mut generator: F) -> Receiver<StreamResult<T>>
where
    T: Default + Send + 'static,
    F: FnMut() -> Result<T> + Send + 'static,
{
    // 有界通道，背压时阻塞生成器而非无限堆积。
    let (out_tx, out_rx) = mpsc::sync_channel(64);
    thread::spawn(move || {
        loop {
            match generator() {
                Ok(item) => {
                    if out_tx
                        .send(StreamResult {
                            Err: None,
                            Item: item,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                Err(err) => {
                    let _ = out_tx.send(StreamResult {
                        Err: Some(err),
                        Item: T::default(),
                    });
                    return;
                }
            }
        }
    });
    out_rx
}

/// 多处共享的收包端：client_loop 与 Finalize 排空共用。
type SharedStreamRx = Arc<Mutex<Receiver<StreamResult<brpb::PrepareSnapshotBackupResponse>>>>;

/// 单 store 的准备流：持有 client、lease 时长与事件出口。
/// 后台 `client_loop` 负责续约与响应分发。
pub struct prepareStream {
    /// 目标 store id。
    pub storeID: u64,
    /// InitConn 后才有值。
    pub cli: Option<Arc<dyn PrepareClient>>,
    /// 续约间隔基准（UpdateLease 的 LeaseInSeconds）。
    pub leaseDuration: Duration,
    /// 向 Preparer 投递事件。
    pub output: SyncSender<event>,

    /// AsyncStreamBy 产生的共享收包端。
    shared_server_stream: Option<SharedStreamRx>,
    /// 后台续约/收包线程句柄。
    client_loop_handle: Option<JoinHandle<Result<()>>>,
    // Finalize 时置位，通知 client_loop 退出。
    stop_bg: Arc<AtomicBool>,
}

impl prepareStream {
    /// 构造未激活流；需 InitConn 才真正建连。
    pub fn new(storeID: u64, output: SyncSender<event>, leaseDuration: Duration) -> Self {
        Self {
            storeID,
            cli: None,
            leaseDuration,
            output,
            shared_server_stream: None,
            client_loop_handle: None,
            stop_bg: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 保存 client、清 stop 标志，并启动 lease 循环。
    /// InitConn initializes the connection to the stream (i.e. "active" the stream).
    pub fn InitConn(&mut self, _ctx: &Context, cli: Arc<dyn PrepareClient>) -> Result<()> {
        self.cli = Some(Arc::clone(&cli));
        self.stop_bg.store(false, Ordering::SeqCst);
        self.GoLeaseLoop(cli, self.leaseDuration)
    }

    /// 委托 stopClientLoop：停后台、发 Finish、排空。
    /// Finalize cuts down this connection and remove the lease.
    pub fn Finalize(&mut self, ctx: &Context) -> Result<()> {
        self.stopClientLoop(ctx)
    }

    /// 未 InitConn 时返回明确错误，避免空指针式失败。
    /// Send forwards a request through the underlying client (used by Preparer).
    pub fn Send(&self, req: &brpb::PrepareSnapshotBackupRequest) -> Result<()> {
        let cli = self
            .cli
            .as_ref()
            .ok_or_else(|| Error::new("prepare stream client not initialized"))?;
        cli.Send(req)
    }

    /// 先同步 UpdateLease 握手，再启动 AsyncStreamBy + client_loop。
    fn GoLeaseLoop(&mut self, cli: Arc<dyn PrepareClient>, dur: Duration) -> Result<()> {
        cli.Send(&brpb::PrepareSnapshotBackupRequest {
            Ty: brpb::PrepareSnapshotBackupRequestType::UpdateLease,
            Regions: Vec::new(),
            LeaseInSeconds: dur.as_secs(),
        })
        // 首包必须是 UpdateLeaseResult，否则视为握手失败。
        .map_err(|err| Error::annotate(err, "failed to initialize the lease"))?;
        let msg = cli
            .Recv()
            .map_err(|err| Error::annotate(err, "failed to recv the initialize lease result"))?;
        if msg.Ty != brpb::PrepareSnapshotBackupEventType::UpdateLeaseResult {
            return Err(Error::new(format!(
                "unexpected type of response during creating lease loop: it is {}",
                msg.Ty
            )));
        }

        let cli_for_stream = Arc::clone(&cli);
        // 把阻塞 Recv 泵到共享 channel，供循环与 Finalize 消费。
        let server_rx = AsyncStreamBy(move || cli_for_stream.Recv());
        let shared_rx: SharedStreamRx = Arc::new(Mutex::new(server_rx));
        self.shared_server_stream = Some(Arc::clone(&shared_rx));

        let store_id = self.storeID;
        let output = self.output.clone();
        let stop_bg = Arc::clone(&self.stop_bg);
        let cli_for_loop = Arc::clone(&cli);
        let shared_rx_loop = Arc::clone(&shared_rx);
        // 后台线程：收包分发 + 周期性 UpdateLease。
        let handle = thread::spawn(move || {
            client_loop(store_id, output, cli_for_loop, shared_rx_loop, stop_bg, dur)
        });
        self.client_loop_handle = Some(handle);
        Ok(())
    }

    /// Finalize 排空路径：把响应转成 event；EOF 由调用方识别。
    fn onResponse(&self, res: StreamResult<brpb::PrepareSnapshotBackupResponse>) -> Result<()> {
        if let Some(err) = res.Err {
            return Err(err);
        }
        let resp = res.Item;
        let (evt, need_deliver) = convert_to_event(self.storeID, &resp);
        if need_deliver {
            let _ = self.output.send(evt);
        }
        Ok(())
    }

    /// 停后台 → join → 发 Finish → 排空直到 EOF/断开。
    /// 后台若已因连接错误退出，超时静默视为已关闭。
    fn stopClientLoop(&mut self, ctx: &Context) -> Result<()> {
        // 先发停止信号，再 join，避免续约线程继续 Send。
        self.stop_bg.store(true, Ordering::SeqCst);

        // 保留后台错误，排空后再返回，避免吞掉 lease 过期。
        // Capture client-loop result (Go: clientLoopHandle.Wait() after drain).
        let mut loop_err: Option<Error> = None;
        if let Some(handle) = self.client_loop_handle.take() {
            match handle.join() {
                Ok(Err(err)) => loop_err = Some(err),
                Ok(Ok(())) => {}
                Err(_) => loop_err = Some(Error::new("client loop panicked")),
            }
        }

        let cli = self
            .cli
            .as_ref()
            .ok_or_else(|| Error::new("prepare stream client not initialized"))?;
        cli.Send(&brpb::PrepareSnapshotBackupRequest {
            // Finish 请求触发对端关闭流。
            Ty: brpb::PrepareSnapshotBackupRequestType::Finish,
            Regions: Vec::new(),
            LeaseInSeconds: 0,
        })
        .map_err(|err| Error::annotate(err, "failed to send finish request"))?;

        let shared = self
            .shared_server_stream
            .as_ref()
            .ok_or_else(|| Error::new("prepare stream server stream not initialized"))?;

        loop {
            if ctx.is_cancelled() {
                return Err(ctx.err().unwrap_or_else(|| Error::new("context canceled")));
            }
            let recv_result = {
                let rx = shared.lock().expect("stream rx poisoned");
                rx.recv_timeout(Duration::from_millis(50))
            };
            match recv_result {
                Ok(res) => match self.onResponse(res) {
                    Err(err) if err.is_eof() || err.message() == "EOF" => break,
                    Err(err) => return Err(err),
                    Ok(()) => {}
                },
                Err(RecvTimeoutError::Timeout) => {
                    // 注入连接错误后生成器可能已退出，静默超时即视为关闭。
                    // AsyncStreamBy may already have exited (e.g. injected conn
                    // error); treat prolonged silence as closed.
                    if loop_err.is_some() {
                        break;
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if let Some(err) = loop_err {
            return Err(err);
        }
        Ok(())
    }
}

/// 后台主循环：短超时收包；到期则 UpdateLease。
/// 连续续约失败超过 lease 时长则投递 misc 错误并退出。
fn client_loop(
    store_id: u64,
    output: SyncSender<event>,
    cli: Arc<dyn PrepareClient>,
    shared_rx: SharedStreamRx,
    // Finalize 时置位，通知 client_loop 退出。
    stop_bg: Arc<AtomicBool>,
    dur: Duration,
) -> Result<()> {
    // 续约节拍约为 lease 的 1/4，与 Go 保持同量级。
    let tick = dur / 4;
    let poll = Duration::from_millis(50).min(tick.max(Duration::from_millis(1)));
    let mut since_tick = Instant::now();
    // last_success 初值刻意很旧，使“距上次成功”的超时判断立即生效。
    let mut last_success = Instant::now()
        .checked_sub(Duration::from_secs(3600 * 24 * 365))
        .unwrap_or_else(Instant::now);

    loop {
        // Finalize 请求退出。
        if stop_bg.load(Ordering::SeqCst) {
            return Ok(());
        }

        let recv_result = {
            let rx = shared_rx.lock().expect("stream rx poisoned");
            rx.recv_timeout(poll)
        };

        match recv_result {
            Ok(res) => {
                // 收包处理失败：投递 misc 并结束循环。
                if let Err(err) = on_response_static(store_id, &output, res) {
                    let err = Error::annotate(err, "failed to recv from the stream");
                    let _ = output.send(event {
                        ty: eventMiscErr,
                        storeID: store_id,
                        region: None,
                        err: Some(err.clone()),
                    });
                    return Err(err);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // 未到续约节拍则继续轮询。
                if since_tick.elapsed() < tick {
                    continue;
                }
                since_tick = Instant::now();
                if let Err(err) = cli.Send(&brpb::PrepareSnapshotBackupRequest {
                    Ty: brpb::PrepareSnapshotBackupRequestType::UpdateLease,
                    Regions: Vec::new(),
                    LeaseInSeconds: dur.as_secs(),
                }) {
                    // 超过一整段 lease 仍续约失败 → 视为过期风险。
                    if last_success.elapsed() > dur {
                        let err = Error::annotate(
                            err,
                            "too many times failed to update the lease, it is probably expired",
                        );
                        let _ = output.send(event {
                            ty: eventMiscErr,
                            storeID: store_id,
                            region: None,
                            err: Some(err.clone()),
                        });
                        return Err(err);
                    }
                } else {
                    // 续约发送成功即刷新成功时刻。
                    last_success = Instant::now();
                }
            }
            // 通道断开视为正常收尾。
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// client_loop 用的无 self 分发：错误直接返回，成功则按需 send event。
fn on_response_static(
    store_id: u64,
    output: &SyncSender<event>,
    res: StreamResult<brpb::PrepareSnapshotBackupResponse>,
) -> Result<()> {
    if let Some(err) = res.Err {
        return Err(err);
    }
    let resp = res.Item;
    let (evt, need_deliver) = convert_to_event(store_id, &resp);
    if need_deliver {
        let _ = output.send(evt);
    }
    Ok(())
}

/// 把 gRPC 响应映射为 event；第二个返回值表示是否需要投递。
/// 有效 UpdateLeaseResult 不投递，避免噪音事件。
fn convert_to_event(store_id: u64, resp: &brpb::PrepareSnapshotBackupResponse) -> (event, bool) {
    match resp.Ty {
        // WaitApply 完成：错误经 convertErr 写入 event.err。
        brpb::PrepareSnapshotBackupEventType::WaitApplyDone => (
            event {
                ty: eventWaitApplyDone,
                storeID: store_id,
                region: resp.Region.clone(),
                err: convertErr(resp.Error.as_ref()),
            },
            true,
        ),
        // lease 无效 → misc+leaseExpired；有效 → 不投递。
        brpb::PrepareSnapshotBackupEventType::UpdateLeaseResult => {
            if !resp.LastLeaseIsValid {
                (
                    event {
                        ty: eventMiscErr,
                        storeID: store_id,
                        region: None,
                        err: Some(leaseExpired()),
                    },
                    true,
                )
            } else {
                (
                    event {
                        ty: eventMiscErr,
                        storeID: store_id,
                        region: None,
                        err: None,
                    },
                    false,
                )
            }
        }
        // 未知类型 → misc+unsupported。
        other => (
            event {
                ty: eventMiscErr,
                storeID: store_id,
                region: None,
                err: Some(Error::annotatef(
                    unsupported(),
                    format!("unknown response type {other} ({})", other as i32),
                )),
            },
            true,
        ),
    }
}
