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

//! Preparer state machine ported from `br/pkg/backup/prepare_snap/prepare.go`.
//!
//! 本文件实现快照备份前的 WaitApply 状态机，语义对齐 Go `prepare.go`。
//! 主路径：建连 → 推进状态 → 循环处理事件，直到覆盖无空洞。
//! Finalize 并行结束各 store 流，并在退出前排空事件通道。
//! 重试把失败区间重新加载 region 后批量发送；次数与退避受公开字段约束。
//! Rust 用 `SyncSender`/`Receiver` 代替 Go channel，用线程代替 errgroup。
//! 空洞检测依赖按 StartKey 排序的成功区间；EndKey 空表示 +inf。
//! 本实现不包含 Go 的 zap MarshalLogObject，日志形态差异不影响控制流。

// BTreeMap 用于有序扫描成功区间；mpsc 承载跨线程事件。
use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// Env 提供 PD/连接；stream 提供 per-store 双向流与事件类型常量。
use crate::env::{Context, Env, PrepareClient, Region, StringifyRangeOf, brpb, metapb};
use crate::errors::{Error, Result, retryLimitExceeded, unsupported};
use crate::stream::{event, eventMiscErr, eventWaitApplyDone, prepareStream};

// defaultMaxRetry × defaultRetryBackoff ≈ 5 分钟上限（与 Go 注释一致）。
// 批量重试时一批只消耗一次重试机会。
const defaultMaxRetry: i32 = 60;
/// 单次重试默认退避间隔。
const defaultRetryBackoff: Duration = Duration::from_secs(5);
/// 默认 lease 时长；建流时写入 `prepareStream`。
const defaultLeaseDur: Duration = Duration::from_secs(120);

/// 按 leader store 聚合的待发送 WaitApply 请求。
type pendingRequests = HashMap<u64, brpb::PrepareSnapshotBackupRequest>;

/// 成功 region 或失败空洞的统一表示；`id==0` 表示纯区间空洞。
#[derive(Clone, Default, Debug)]
struct rangeOrRegion {
    /// Region id；空洞为 0。
    id: u64,
    /// 区间起点（含）。
    startKey: Vec<u8>,
    /// 区间终点（不含）；空表示正无穷。
    endKey: Vec<u8>,
}

impl rangeOrRegion {
    /// 日志友好字符串，对齐 Go `rangeOrRegion.String`。
    fn String(&self) -> String {
        let rng = StringifyRangeOf(&self.startKey, &self.endKey);
        if self.id == 0 {
            return format!("range{rng}");
        }
        format!("region(id={}, range={rng})", self.id)
    }
}

impl std::fmt::Display for rangeOrRegion {
    /// 委托 `String()`，方便 `format!` / 错误注解复用同一文案。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.String())
    }
}

/// Preparer drives prepare-snapshot wait-apply across stores.
///
/// 跨 store 驱动 WaitApply：维护 inflight、失败区间与已完成覆盖。
/// 对外可调 `RetryBackoff` / `RetryLimit` / `LeaseDuration`。
/// `AfterConnectionsEstablished` 供测试在建连后、驱动前插入钩子。
pub struct Preparer {
    /// PD/TiKV 环境抽象。
    env: Arc<dyn Env>,

    /// 已发送尚未确认的 WaitApply（按 region id）。
    inflightReqs: HashMap<u64, metapb::Region>,
    /// 待重试的失败区间或空洞。
    failed: Vec<rangeOrRegion>,
    /// 已成功 WaitApply 的区间，按 StartKey 有序以便查空洞。
    waitApplyDoneRegions: BTreeMap<Vec<u8>, rangeOrRegion>,
    /// 已消耗的重试次数。
    retryTime: i32,
    /// 下一次允许重试的时刻。
    nextRetryAt: Option<Instant>,
    /// 是否已安排退避后的重试（辅助状态）。
    retryBackoffPending: bool,

    /// 各 prepareStream 共用的事件发送端。
    event_tx: SyncSender<event>,
    /// 主循环读取事件的接收端（Mutex 以便与 Finalize 并发排空）。
    event_rx: Mutex<mpsc::Receiver<event>>,
    /// store id → 已缓存的双向流。
    clients: HashMap<u64, prepareStream>,

    /// 全覆盖完成标志；DriveLoop 退出条件。
    waitApplyFinished: bool,

    /// 公开：失败后重试退避。
    pub RetryBackoff: Duration,
    /// 公开：最大重试次数。
    pub RetryLimit: i32,
    /// 公开：lease 时长，传给流初始化。
    pub LeaseDuration: Duration,

    /// 公开：全部连接建立后的可选钩子（测试用）。
    pub AfterConnectionsEstablished: Option<Box<dyn Fn() + Send + Sync>>,
}

/// Construct a new Preparer (Go `New`).
/// 使用默认重试/lease，并创建容量 128 的同步事件通道。
pub fn New(env: Arc<dyn Env>) -> Preparer {
    let (event_tx, event_rx) = mpsc::sync_channel(128);
    Preparer {
        env,
        inflightReqs: HashMap::new(),
        failed: Vec::new(),
        waitApplyDoneRegions: BTreeMap::new(),
        retryTime: 0,
        nextRetryAt: None,
        retryBackoffPending: false,
        event_tx,
        event_rx: Mutex::new(event_rx),
        clients: HashMap::new(),
        waitApplyFinished: false,
        RetryBackoff: defaultRetryBackoff,
        RetryLimit: defaultMaxRetry,
        LeaseDuration: defaultLeaseDur,
        AfterConnectionsEstablished: None,
    }
}

impl Preparer {
    /// 查询 WaitApply 是否已完成全覆盖。
    pub fn wait_apply_finished(&self) -> bool {
        self.waitApplyFinished
    }

    /// DriveLoopAndWaitPrepare drives the state machine until snapshot is safe.
    /// 重置重试计数 → 建连 → 可选钩子 → AdvanceState → 事件循环直至完成。
    pub fn DriveLoopAndWaitPrepare(&mut self, ctx: &Context) -> Result<()> {
        self.retryTime = 0;
        // 建连失败直接返回，避免在无流时进入事件循环。
        self.PrepareConnections(ctx)
            .map_err(|err| Error::annotate(err, "failed to prepare connections"))?;
        if let Some(hook) = &self.AfterConnectionsEstablished {
            // 钩子仅用于测试注入，生产路径通常为 None。
            hook();
        }
        // 首次推进：可能立即发现全空覆盖并触发首轮 WaitApply。
        self.AdvanceState(ctx)
            .map_err(|err| Error::annotate(err, "failed to begin step"))?;
        while !self.waitApplyFinished {
            // 每步处理事件后再 AdvanceState，直到无空洞。
            self.WaitAndHandleNextEvent(ctx)
                .map_err(|err| Error::annotate(err, "failed to step"))?;
        }
        Ok(())
    }

    /// Finalize notifies the cluster to return to normal mode.
    /// 并行 Finalize 各流（对齐 Go errgroup），同时排空事件以免 lease 过期被吞掉。
    pub fn Finalize(&mut self, ctx: &Context) -> Result<()> {
        let err_slots: Arc<Mutex<Vec<Option<Error>>>> = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();

        // Move clients out and finalize in parallel threads (Go errgroup).
        // take 走 clients，避免与事件循环争用所有权。
        let mut clients = std::mem::take(&mut self.clients);
        for (id, mut stream) in clients.drain() {
            let err_slots = Arc::clone(&err_slots);
            let ctx = ctx.clone();
            handles.push(thread::spawn(move || {
                let res = stream.Finalize(&ctx).map_err(|err| {
                    Error::annotatef(
                        err,
                        format!("failed to finalize the prepare stream for {id}"),
                    )
                });
                if let Err(err) = res {
                    err_slots.lock().expect("poison").push(Some(err));
                }
            }));
        }

        // 汇聚线程：join 全部 Finalize，再投递首个错误或成功。
        let (done_tx, done_rx) = mpsc::channel::<Option<Error>>();
        thread::spawn(move || {
            for h in handles {
                let _ = h.join();
            }
            let errs = err_slots.lock().expect("poison");
            let first = errs.iter().find_map(|e| e.clone());
            let _ = done_tx.send(first);
        });

        // Match Go Preparer.Finalize: drain eventChan until streams finish, then
        // treat channel-close (no more producers) and return. Lease-expired and
        // other misc errors delivered during finalize must surface via onEvent.
        // 在流结束前持续 onEvent；结束后排空缓冲再返回。
        let mut finalize_done = false;
        loop {
            if ctx.is_cancelled() {
                return Err(ctx.err().unwrap_or_else(|| Error::new("context canceled")));
            }

            if !finalize_done {
                match done_rx.try_recv() {
                    Ok(Some(err)) => return Err(err),
                    Ok(None) => {
                        // All streams finalized (Go: close(p.eventChan)). Drain any
                        // buffered events, then return.
                        // 对齐 Go：流结束后先排空再 Ok。
                        finalize_done = true;
                        loop {
                            let evt = {
                                let rx = self.event_rx.lock().expect("event rx poisoned");
                                match rx.try_recv() {
                                    Ok(evt) => Some(evt),
                                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => {
                                        None
                                    }
                                }
                            };
                            match evt {
                                Some(evt) => self.onEvent(ctx, evt)?,
                                None => return Ok(()),
                            }
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        // 汇聚线程已退出且无错误。
                        return Ok(());
                    }
                }
            }

            // 短超时轮询：兼顾取消检查与事件处理。
            let evt = {
                let rx = self.event_rx.lock().expect("event rx poisoned");
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(evt) => Some(evt),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return Ok(()),
                }
            };
            if let Some(evt) = evt {
                self.onEvent(ctx, evt)?;
            }
        }
    }

    /// 非阻塞排空当前可读事件，减少主循环系统调用次数。
    fn batchEvents(&self, evts: &mut Vec<event>) {
        let rx = self.event_rx.lock().expect("event rx poisoned");
        loop {
            match rx.try_recv() {
                Ok(evt) => evts.push(evt),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return,
            }
        }
    }

    /// WaitAndHandleNextEvent waits for the next prepare event (exported for tests).
    /// 在超时与 `nextRetryAt` 之间取较小等待；到期则走 `workOnPendingRanges`。
    pub fn WaitAndHandleNextEvent(&mut self, ctx: &Context) -> Result<()> {
        loop {
            if ctx.is_cancelled() {
                return Err(ctx.err().unwrap_or_else(|| Error::new("context canceled")));
            }

            // 有计划重试时，超时不超过距重试点的剩余时间。
            let timeout = if let Some(at) = self.nextRetryAt {
                let until_retry = at.saturating_duration_since(Instant::now());
                if until_retry.is_zero() {
                    return self.workOnPendingRanges(ctx);
                }
                until_retry.min(Duration::from_millis(100))
            } else {
                Duration::from_millis(100)
            };

            let recv = {
                let rx = self.event_rx.lock().expect("event rx poisoned");
                rx.recv_timeout(timeout.max(Duration::from_millis(1)))
            };

            match recv {
                Ok(evt) => {
                    // 批量取出后续事件，再统一 AdvanceState。
                    let mut events = vec![evt];
                    self.batchEvents(&mut events);
                    for evt in events {
                        self.onEvent(ctx, evt).map_err(|err| {
                            Error::annotatef(err, "failed to handle event".into())
                        })?;
                    }
                    return self.AdvanceState(ctx);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(at) = self.nextRetryAt {
                        if Instant::now() >= at {
                            return self.workOnPendingRanges(ctx);
                        }
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::new("event channel disconnected"));
                }
            }
        }
    }

    /// 仅当 region id 与 epoch 完全匹配时移除 inflight，防止过期响应误消。
    fn removePendingRequest(&mut self, r: &metapb::Region) -> bool {
        let Some(r2) = self.inflightReqs.get(&r.Id) else {
            return false;
        };
        // Go's generated protobuf getters return zero for both a nil epoch and
        // an explicitly present zero-valued epoch. Compare the getter values,
        // rather than the Rust `Option` shape, to preserve that behavior.
        let epoch_values = |epoch: &Option<metapb::RegionEpoch>| {
            epoch
                .as_ref()
                .map(|value| (value.Version, value.ConfVer))
                .unwrap_or((0, 0))
        };
        let matches = epoch_values(&r2.RegionEpoch) == epoch_values(&r.RegionEpoch);
        if !matches {
            return false;
        }
        self.inflightReqs.remove(&r.Id);
        true
    }

    /// 分发事件：misc 错误不可恢复；WaitApplyDone 更新覆盖或进入失败重试。
    fn onEvent(&mut self, _ctx: &Context, e: event) -> Result<()> {
        match e.ty {
            x if x == eventMiscErr => {
                // 流级不可恢复错误，带上 store id 后上抛。
                let err = e.err.unwrap_or_else(|| Error::new("misc error"));
                Err(Error::annotatef(
                    err,
                    format!("unrecoverable error at store {}", e.storeID),
                ))
            }
            x if x == eventWaitApplyDone => {
                let Some(region) = e.region else {
                    return Ok(());
                };
                if !self.removePendingRequest(&region) {
                    // unmatched / stale response
                    // 过期/不匹配响应直接忽略，避免污染覆盖集。
                    return Ok(());
                }
                let r = rangeOrRegion {
                    id: region.Id,
                    startKey: region.StartKey.clone(),
                    endKey: region.EndKey.clone(),
                };
                if let Some(err) = e.err {
                    let _ = err;
                    // 单 region 失败：记入 failed，安排退避重试。
                    self.failed.push(r);
                    self.nextRetryAt = Some(Instant::now() + self.RetryBackoff);
                    self.retryBackoffPending = true;
                    return Ok(());
                }
                if let Some(old) = self
                    .waitApplyDoneRegions
                    .insert(r.startKey.clone(), r.clone())
                {
                    let _ = old;
                    // overlapping in success region
                    // 同 StartKey 覆盖写：保留最新成功结果。
                }
                Ok(())
            }
            other => Err(Error::annotatef(
                unsupported(),
                format!("unsupported event type {other}"),
            )),
        }
    }

    /// AdvanceState checks whether wait-apply is finished (exported for tests).
    /// inflight 与 failed 皆空时查空洞；无空洞则完成，否则把空洞当作失败区间重试。
    pub fn AdvanceState(&mut self, ctx: &Context) -> Result<()> {
        if self.inflightReqs.is_empty() && self.failed.is_empty() {
            let holes = self.checkHole();
            if holes.is_empty() {
                self.waitApplyFinished = true;
                return Ok(());
            }
            self.failed = holes;
            return self.workOnPendingRanges(ctx);
        }
        Ok(())
    }

    /// 扫描已成功区间，找出键空间空洞（含首尾）。
    /// 空成功集时返回整域空洞（default rangeOrRegion）。
    fn checkHole(&self) -> Vec<rangeOrRegion> {
        if self.waitApplyDoneRegions.is_empty() {
            return vec![rangeOrRegion::default()];
        }
        let mut last: Vec<u8> = Vec::new();
        let mut failed = Vec::new();
        for item in self.waitApplyDoneRegions.values() {
            // last < start 说明中间有缺口。
            if last.as_slice() < item.startKey.as_slice() {
                failed.push(rangeOrRegion {
                    id: 0,
                    startKey: last.clone(),
                    endKey: item.startKey.clone(),
                });
            }
            last = item.endKey.clone();
        }
        // 末尾 EndKey 非空则后面还有到 +inf 的空洞。
        if !last.is_empty() {
            failed.push(rangeOrRegion {
                id: 0,
                startKey: last,
                endKey: Vec::new(),
            });
        }
        failed
    }

    /// 对 failed 区间重新 LoadRegions 并发送 WaitApply；超过 RetryLimit 则失败。
    fn workOnPendingRanges(&mut self, ctx: &Context) -> Result<()> {
        self.nextRetryAt = None;
        self.retryBackoffPending = false;
        if self.failed.is_empty() {
            return Ok(());
        }
        self.retryTime += 1;
        if self.retryTime > self.RetryLimit {
            return Err(retryLimitExceeded());
        }

        let mut preqs: pendingRequests = HashMap::new();
        let pending = std::mem::take(&mut self.failed);
        for r in pending {
            // 按当前 PD 视图重新解析区间内 region（拓扑可能已变）。
            let rs = self
                .env
                .LoadRegionsInKeyRange(ctx, &r.startKey, &r.endKey)
                .map_err(|err| {
                    Error::annotatef(
                        err,
                        format!(
                            "retrying range of {}: get region",
                            StringifyRangeOf(&r.startKey, &r.endKey)
                        ),
                    )
                })?;
            for region in rs {
                self.pushWaitApply(&mut preqs, region.as_ref());
            }
        }
        self.sendWaitApply(ctx, preqs)
    }

    /// 按 store 发送聚合后的 WaitApply；缺流则先 `streamOf` 建连。
    fn sendWaitApply(&mut self, ctx: &Context, reqs: pendingRequests) -> Result<()> {
        for (store, req) in reqs {
            // 懒建流：重试路径可能触及尚未缓存的 store。
            self.streamOf(ctx, store)?;
            let stream = self
                .clients
                .get(&store)
                .ok_or_else(|| Error::new(format!("stream missing for store {store}")))?;
            // Send 失败视为该 store 通信故障，带 store id 上抛。
            stream.Send(&req).map_err(|err| {
                Error::annotatef(err, format!("failed to send message to the store {store}"))
            })?;
        }
        Ok(())
    }

    /// 确保指定 store 的流已缓存；否则 Connect + InitConn。
    fn streamOf(&mut self, ctx: &Context, storeID: u64) -> Result<()> {
        if self.clients.contains_key(&storeID) {
            return Ok(());
        }
        let cli = self
            .env
            .ConnectToStore(ctx, storeID)
            .map_err(|err| Error::annotatef(err, format!("failed to dial store {storeID}")))?;
        self.createAndCacheStream(ctx, cli, storeID).map_err(|err| {
            Error::annotatef(
                err,
                format!("failed to create and cache stream for store {storeID}"),
            )
        })
    }

    /// 创建 `prepareStream`、InitConn（含 lease 续约循环）并写入 clients。
    fn createAndCacheStream(
        &mut self,
        ctx: &Context,
        cli: Arc<dyn PrepareClient>,
        storeID: u64,
    ) -> Result<()> {
        // 双重检查：PrepareConnections 与懒建流可能并发路径调用。
        if self.clients.contains_key(&storeID) {
            return Ok(());
        }
        // LeaseDuration 注入流，供后台 UpdateLease 使用。
        let mut s = prepareStream::new(storeID, self.event_tx.clone(), self.LeaseDuration);
        // InitConn 失败不得写入 clients，避免半初始化流。
        s.InitConn(ctx, cli)?;
        self.clients.insert(storeID, s);
        Ok(())
    }

    /// 按 leader store 聚合 WaitApply，并登记 inflight。
    fn pushWaitApply(&mut self, reqs: &mut pendingRequests, region: &dyn Region) {
        // 只发给 leader store，与 TiKV PrepareSnapshot 路由一致。
        let leader = region.GetLeaderStoreID();
        let meta = region.GetMeta();
        let entry = reqs
            .entry(leader)
            .or_insert_with(|| brpb::PrepareSnapshotBackupRequest {
                Ty: brpb::PrepareSnapshotBackupRequestType::WaitApply,
                Regions: Vec::new(),
                // WaitApply 本身不携带 lease；lease 由流上的 UpdateLease 维护。
                LeaseInSeconds: 0,
            });
        entry.Regions.push(meta.clone());
        // 登记 inflight，供响应 epoch 匹配时清除。
        self.inflightReqs.insert(meta.Id, meta);
    }

    /// PrepareConnections prepares connections and pauses admin commands per store.
    /// 枚举 live stores，全部建流后才返回，保证后续 WaitApply 有通道可用。
    pub fn PrepareConnections(&mut self, ctx: &Context) -> Result<()> {
        let stores = self
            .env
            .GetAllLiveStores(ctx)
            .map_err(|err| Error::annotate(err, "failed to get all live stores"))?;

        // 先收集全部 client，再统一建流，避免半开状态难回滚。
        let mut clients: HashMap<u64, Arc<dyn PrepareClient>> = HashMap::new();
        for store in &stores {
            let cli = self.env.ConnectToStore(ctx, store.Id).map_err(|err| {
                Error::annotatef(err, format!("failed to dial the store {}", store.Id))
            })?;
            clients.insert(store.Id, cli);
        }

        for (id, cli) in clients {
            // InitConn 会启动收包/续约；任一门店失败则整次 PrepareConnections 失败。
            self.createAndCacheStream(ctx, cli, id).map_err(|err| {
                Error::annotatef(
                    err,
                    format!("failed to create and cache stream for store {id}"),
                )
            })?;
        }
        Ok(())
    }
}
