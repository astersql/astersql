// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Region 检查点收集器：把带 leader 的 region 批发给对应 TiKV，汇总 flush TS。
//!
//! 对齐 Go `br/pkg/streamhelper/collector.go`。生命周期短于 advancer 的一次 tick：
//! 按 store 懒创建 `StoreCollector` 工作线程，批量调用 `GetLastFlushTSOfRegion`；
//! 服务端支持离散 region 批量，故请求区间不必连续。集群级入口为 `ClusterCollector`。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::advancer_env::Env;
use crate::regioniter::RegionWithLeader;
use crate::stubs::{GetLastFlushTSOfRegionRequest, KeyRange, RegionIdentity};

/// 单次 RPC 批量 region 数上限；与 Go `defaultBatchSize` 一致，兼顾吞吐与请求体大小。
pub const defaultBatchSize: usize = 1024;

/// 某 region 成功拿到检查点时的回调：参数为 checkpoint TS 与 key 范围。
pub type OnSuccessHook = Arc<dyn Fn(u64, KeyRange) + Send + Sync>;

/// 单 store 收集器：从 channel 收 region，批处理并请求该 store 的 last flush TS。
///
/// 字段中 `checkpoint` / `inconsistent` / `regionMap` 应只由 `recv_loop` 线程写入；
/// 循环结束后主线程再读，与 Go 侧并发约定一致。临时结构，advancer tick 结束即销毁。
struct StoreCollector {
    storeID: u64,
    batchSize: usize,
    service: Arc<dyn Env>,
    rx: Mutex<Option<Receiver<RegionWithLeader>>>,
    /// 一次性错误槽：已有错误则忽略后续 report，避免覆盖根因。
    err: Mutex<Option<String>>,
    /// 工作线程是否已退出；主路径用其判断是否还能投递。
    done: AtomicBool,
    onSuccess: Option<OnSuccessHook>,
    /// 尚未发出的 `GetLastFlushTSOfRegion` 请求体（离散 region 列表）。
    currentRequest: Mutex<GetLastFlushTSOfRegionRequest>,
    /// 本 store 上已观察到的最小成功 checkpoint（0 表示尚未成功）。
    checkpoint: Mutex<u64>,
    /// RPC 返回 Err 的 region 对应 key 范围，供上层缩小重扫。
    inconsistent: Mutex<Vec<KeyRange>>,
    regionMap: Mutex<HashMap<u64, KeyRange>>,
}

impl StoreCollector {
    /// 仅首次写入错误；后续重复 report 直接丢弃（对齐 Go `reportErr`）。
    fn report_err(&self, err: String) {
        let mut g = self.err.lock().unwrap();
        if g.is_some() {
            return;
        }
        *g = Some(err);
    }

    /// 缓存 region id → key range，供 RPC 结果回填成功回调或不一致区间。
    fn append_region_map(&self, r: &RegionWithLeader) {
        self.regionMap.lock().unwrap().insert(
            r.Region.GetId(),
            KeyRange {
                StartKey: r.Region.StartKey.clone(),
                EndKey: r.Region.EndKey.clone(),
            },
        );
    }

    /// 冲刷当前批次：调用 `GetLastFlushTSOfRegion`，更新最小检查点或不一致范围。
    ///
    /// RPC 失败时 `ClearCache` 禁用连接缓存后返回错误。检查点为 0 作占位：
    /// 成功路径取严格更小的 TS（或首次非零），与 Go「checkpoint 永不为 0」假设一致。
    fn send_pending(&self) -> Result<(), String> {
        let mut req = self.currentRequest.lock().unwrap();
        let cli = self.service.GetLogBackupClient(self.storeID)?;
        let cps = match cli.GetLastFlushTSOfRegion(&req) {
            Ok(v) => v,
            Err(e) => {
                let _ = self.service.ClearCache(self.storeID);
                return Err(e);
            }
        };
        req.Regions.clear();
        drop(req);
        for checkpoint in cps.Checkpoints {
            if checkpoint.Err.is_some() {
                // epoch/leader 等不一致：记入 FailureSubRanges，供上层重试扫描。
                let kr = self
                    .regionMap
                    .lock()
                    .unwrap()
                    .get(&checkpoint.Region.Id)
                    .cloned()
                    .unwrap_or_default();
                self.inconsistent.lock().unwrap().push(kr);
            } else {
                if let Some(hook) = &self.onSuccess {
                    let kr = self
                        .regionMap
                        .lock()
                        .unwrap()
                        .get(&checkpoint.Region.Id)
                        .cloned()
                        .unwrap_or_default();
                    hook(checkpoint.Checkpoint, kr);
                }
                let mut cp = self.checkpoint.lock().unwrap();
                if checkpoint.Checkpoint < *cp || *cp == 0 {
                    *cp = checkpoint.Checkpoint;
                }
            }
        }
        Ok(())
    }

    /// 接收循环：满批或 channel 关闭时冲刷；出错则 report 并退出，最后置 `done`。
    fn recv_loop(self: &Arc<Self>) {
        let rx = match self.rx.lock().unwrap().take() {
            Some(r) => r,
            None => return,
        };
        loop {
            match rx.recv() {
                Ok(r) => {
                    self.append_region_map(&r);
                    // 附带 epoch，便于 store 侧检测分裂/合并导致的 EpochNotMatch。
                    let should_flush = {
                        let mut req = self.currentRequest.lock().unwrap();
                        req.Regions.push(RegionIdentity {
                            Id: r.Region.GetId(),
                            EpochVersion: r.Region.GetRegionEpoch().Version,
                        });
                        req.Regions.len() >= self.batchSize
                    };
                    if should_flush {
                        if let Err(e) = self.send_pending() {
                            self.report_err(e);
                            break;
                        }
                    }
                }
                Err(_) => {
                    // 发送端关闭：冲刷尾部批次后结束（对齐 Go input channel closed）。
                    if let Err(e) = self.send_pending() {
                        self.report_err(e);
                    }
                    break;
                }
            }
        }
        self.done.store(true, Ordering::SeqCst);
    }
}

/// 聚合后的检查点结果：全局最小成功 TS，以及失败/无 leader 子区间。
#[derive(Clone, Debug, Default)]
pub struct StoreCheckpoints {
    /// 是否至少有一个成功的 region 检查点。
    pub HasCheckpoint: bool,
    /// 各成功 region 中的最小 checkpoint TS（全局安全水位候选）。
    pub Checkpoint: u64,
    /// 无 leader 或 RPC 报错的子区间，需后续重试。
    pub FailureSubRanges: Vec<KeyRange>,
}

impl StoreCheckpoints {
    /// 合并另一份结果：取更小的成功 checkpoint，并拼接失败区间。
    pub fn merge(&mut self, other: StoreCheckpoints) {
        if other.HasCheckpoint && (other.Checkpoint < self.Checkpoint || !self.HasCheckpoint) {
            self.Checkpoint = other.Checkpoint;
            self.HasCheckpoint = true;
        }
        self.FailureSubRanges.extend(other.FailureSubRanges);
    }
}

/// 调试友好展示：检查点 TS 或 `none`，并附带剩余失败区间个数。
impl std::fmt::Display for StoreCheckpoints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StoreCheckpoints:")?;
        if self.HasCheckpoint {
            write!(f, "{}", self.Checkpoint)?;
        } else {
            write!(f, "none")?;
        }
        write!(f, ":(remaining {} ranges)", self.FailureSubRanges.len())
    }
}

/// 运行中的单 store 收集器句柄：持有 worker 线程与输入 channel。
/// `Finish` 时先 drop `input` 再 join，保证尾批冲刷完成。
struct RunningStoreCollector {
    collector: Arc<StoreCollector>,
    join: Option<JoinHandle<()>>,
    /// 向 worker 投递 region；置 `None` 即关闭 channel。
    input: Option<Sender<RegionWithLeader>>,
}

/// 集群级控制器：按 leader store 懒创建多个 `StoreCollector` 并汇总结果。
///
/// 对齐 Go `clusterCollector`；Rust 用 `cancelled` 近似 masterCtx 取消语义，
/// 无 leader 的 region 记入 `noLeaders`，最终并入 `FailureSubRanges`。
pub struct ClusterCollector {
    /// store id → 运行中的单 store 收集器。
    collectors: HashMap<u64, RunningStoreCollector>,
    /// Collect 时发现无 leader 的区间，Finish 时并入失败列表。
    noLeaders: Vec<KeyRange>,
    onSuccess: Option<OnSuccessHook>,
    /// 错误或 Finish 后置位，阻止继续投递（近似 Go cancel）。
    cancelled: AtomicBool,
    srv: Arc<dyn Env>,
}

/// 创建空的集群收集器；须先 `CollectRegion` 再 `Finish` 取得所有权并返回结果。
pub fn NewClusterCollector(srv: Arc<dyn Env>) -> ClusterCollector {
    ClusterCollector {
        collectors: HashMap::new(),
        noLeaders: Vec::new(),
        onSuccess: None,
        cancelled: AtomicBool::new(false),
        srv,
    }
}

impl ClusterCollector {
    /// 设置成功拿到检查点时的钩子；须在开始收集前调用，以便新 store worker 继承。
    pub fn SetOnSuccessHook(&mut self, hook: OnSuccessHook) {
        self.onSuccess = Some(hook);
    }

    /// 将 region 投递给其 leader store 的收集线程；无 leader 则记入失败范围。
    ///
    /// 已取消时静默成功返回（对齐 Go masterCtx 已取消）。若对应 worker 已因错误结束，
    /// 则置取消并向上返回该错误。
    pub fn CollectRegion(&mut self, r: RegionWithLeader) -> Result<(), String> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Ok(());
        }
        // 无 leader：无法路由到 store，记失败区间，不阻断其它 region 收集。
        if r.Leader.GetStoreId() == 0 {
            self.noLeaders.push(KeyRange {
                StartKey: r.Region.StartKey.clone(),
                EndKey: r.Region.EndKey.clone(),
            });
            return Ok(());
        }
        let leader = r.Leader.StoreId;
        // 按 leader store 懒启动 worker，避免为未触及的 store 建连接。
        if !self.collectors.contains_key(&leader) {
            let (tx, rx) = mpsc::channel();
            let arc = Arc::new(StoreCollector {
                storeID: leader,
                batchSize: defaultBatchSize,
                service: self.srv.clone(),
                rx: Mutex::new(Some(rx)),
                err: Mutex::new(None),
                done: AtomicBool::new(false),
                onSuccess: self.onSuccess.clone(),
                currentRequest: Mutex::new(GetLastFlushTSOfRegionRequest::default()),
                checkpoint: Mutex::new(0),
                inconsistent: Mutex::new(Vec::new()),
                regionMap: Mutex::new(HashMap::new()),
            });
            let worker = arc.clone();
            let join = thread::spawn(move || worker.recv_loop());
            self.collectors.insert(
                leader,
                RunningStoreCollector {
                    collector: arc,
                    join: Some(join),
                    input: Some(tx),
                },
            );
        }
        let sc = self.collectors.get_mut(&leader).unwrap();
        if sc.collector.done.load(Ordering::SeqCst) {
            if let Some(err) = sc.collector.err.lock().unwrap().clone() {
                self.cancelled.store(true, Ordering::SeqCst);
                return Err(err);
            }
        }
        sc.input
            .as_ref()
            .ok_or_else(|| "collector channel closed".to_string())?
            .send(r)
            .map_err(|_| "collector channel closed".to_string())
    }

    /// 结束收集：关闭各 store 输入、join worker，合并检查点；消费 self 所有权。
    ///
    /// 与 Go `Finish` 相同：调用后不可再复用，下次 tick 需新建收集器。
    pub fn Finish(mut self) -> Result<StoreCheckpoints, String> {
        self.cancelled.store(true, Ordering::SeqCst);
        let mut result = StoreCheckpoints {
            FailureSubRanges: self.noLeaders.clone(),
            ..Default::default()
        };
        for (id, mut coll) in self.collectors.drain() {
            // 关闭 Sender 触发 recv_loop 冲刷尾部并退出。
            drop(coll.input.take());
            if let Some(j) = coll.join.take() {
                let _ = j.join();
            }
            if let Some(err) = coll.collector.err.lock().unwrap().clone() {
                return Err(format!("store {id}: {err}"));
            }
            let cp = *coll.collector.checkpoint.lock().unwrap();
            let inconsistent = coll.collector.inconsistent.lock().unwrap().clone();
            result.merge(StoreCheckpoints {
                HasCheckpoint: cp != 0,
                Checkpoint: cp,
                FailureSubRanges: inconsistent,
            });
        }
        Ok(result)
    }
}
