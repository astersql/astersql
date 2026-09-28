// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! PD 仿真器核心：静态 region 布局 + 全局 checkpoint 上传/等待。
//! 对应 Go `pd_sim.go`，供 checkpoint advancer / CRR harness 单测使用。
//! 布局创建后不支持 split/merge；Scatter 仅做 store 重分配并重置 checkpoint。
//! 内部用 Mutex+Condvar 同步 checkpoint 世代，等待方可被 Context 取消。
//! 底层委托 fakecluster；本文件负责边界校验、任务绑定与 TSO/快照查询封装。
//! 未知任务名与 checkpoint 回滚均返回可断言的错误字符串。
//! 服务侧 trait 实现见 `pd_sim_service.rs`，本文件聚焦状态机本身。

// Ordering/Condvar：checkpoint 世代与取消通知跨线程可见。
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

// fakecluster 提供 Store/Region/TSO；oracle::ComposeTS 拼物理+逻辑时间。
use astersql_br_pkg_utiltest_fakecluster::{self as fakecluster, Context as FcContext, oracle};

use crate::stubs::{Context, Error, Result};
use crate::types::{
    DeterministicRNG, RegionBoundary, RegionState, TestContext, defaultTaskName,
    defaultTaskStartPhysical,
};

/// PDSim 可变状态：任务绑定、全局 checkpoint 与确定性 RNG。
pub(crate) struct PDSimState {
    /// 唯一绑定任务名；上传/查询/清空均按名校验。
    pub(crate) task_name: String,
    /// 任务起始 TSO，写入 Begin 事件与 region 初始 checkpoint。
    pub(crate) task_start: u64,
    /// advancer 已上传的全局安全点；Scatter 迁出时用作回退基准。
    pub(crate) global_checkpoint: u64,
    /// Generation counter bumped on each Upload; waiters observe via Condvar.
    /// 每次成功上传递增，供 Wait 区分“已推进”与超时空转。
    pub(crate) checkpoint_gen: u64,
    pub(crate) rng: DeterministicRNG,
}

/// PDSim simulates PD interfaces used by the checkpoint advancer.
/// Region layout is static: no split/merge in this simulator.
/// PD 仿真门面：持有 fakecluster 与受锁保护的 checkpoint 状态。
pub struct PDSim {
    /// 静态拓扑与 TSO/GC 行为的真实载体。
    pub cluster: fakecluster::Cluster,
    /// 任务与全局 checkpoint；与 checkpoint_cv 配对使用。
    pub(crate) state: Mutex<PDSimState>,
    /// Upload 成功后唤醒 WaitGlobalCheckpointAdvance。
    pub(crate) checkpoint_cv: Condvar,
}

/// 用 TestContext 构造 PDSim：空布局回退单 region；空任务名用默认值。
/// 校验边界后物化 store/region，并开启 flush sub 支持、关闭 legacy RPC。
pub fn NewPDSimWithTestContext(
    mut boundaries: Vec<RegionBoundary>,
    mut taskName: String,
    tc: &TestContext,
) -> Result<Arc<PDSim>> {
    // 空边界时造一个覆盖全范围的单 region，避免调用方必传布局。
    if boundaries.is_empty() {
        boundaries = vec![RegionBoundary {
            StoreID: 1,
            ..Default::default()
        }];
    }
    if taskName.is_empty() {
        taskName = defaultTaskName.to_string();
    }
    validateBoundaries(&boundaries)?;

    // 任务起始 TS = 默认物理时间 + 小抖动，保证跨用例不完全撞车。
    let mut rng = tc.RNG("pd-sim");
    let task_start = oracle::ComposeTS(defaultTaskStartPhysical + rng.Int63n(1 << 20), 0);

    let p = Arc::new(PDSim {
        cluster: fakecluster::New(),
        state: Mutex::new(PDSimState {
            task_name: taskName,
            task_start,
            global_checkpoint: 0,
            checkpoint_gen: 0,
            rng,
        }),
        checkpoint_cv: Condvar::new(),
    });
    // 时钟锚定到任务起始，后续 AllocTSO 从此基线单调递增。
    p.cluster.SetCurrentTS(task_start);

    for b in &boundaries {
        let store = p.cluster.EnsureStore(b.StoreID, 1);
        // DRR 路径依赖 flush subscription；legacy region checkpoint RPC 关闭。
        store.SetSupportFlushSub(true);
        store
            .LegacyRegionCheckpointRPCEnabled
            .store(0, Ordering::SeqCst);
        *store.FlushTaskName.lock().unwrap() = "drr".to_string();

        // 初始 epoch=1、checkpoint=task_start；非 pending 状态。
        let region = fakecluster::NewRegion(
            p.cluster.AllocID(),
            b.StartKey.clone(),
            b.EndKey.clone(),
            b.StoreID,
            1,
            task_start,
            false,
        );
        // peers 列表仅含所属 store，模拟单副本测试拓扑。
        p.cluster.AddRegion(region, &[b.StoreID]);
    }
    Ok(p)
}

/// 校验全范围可扫描：首空 StartKey、末空 EndKey、相邻首尾相接且有序。
fn validateBoundaries(boundaries: &[RegionBoundary]) -> Result<()> {
    for (i, b) in boundaries.iter().enumerate() {
        if b.StoreID == 0 {
            return Err(Error::new(format!("region[{i}] has empty store id")));
        }
        if i == 0 {
            // 全范围扫描要求 region[0] 从空键起。
            if !b.StartKey.is_empty() {
                return Err(Error::new(
                    "region[0] must start from empty key for full-range scan",
                ));
            }
            continue;
        }
        let prev = &boundaries[i - 1];
        // StartKey 必须按字节序非降序排列。
        if prev.StartKey.as_slice() > b.StartKey.as_slice() {
            return Err(Error::new(format!("region[{i}] start key is not sorted")));
        }
        // 相邻 region 必须首尾相接，禁止空洞或重叠。
        if prev.EndKey != b.StartKey {
            return Err(Error::new(format!(
                "region[{}] end key does not connect region[{i}] start key",
                i - 1
            )));
        }
    }
    // 末段 EndKey 为空才覆盖到 +∞。
    if !boundaries[boundaries.len() - 1].EndKey.is_empty() {
        return Err(Error::new(
            "last region must end with empty key for full-range scan",
        ));
    }
    Ok(())
}

impl PDSim {
    /// AllocTSO allocates a monotonically increasing TSO.
    /// 分配单调递增 TSO，委托 fakecluster 时钟。
    pub fn AllocTSO(&self) -> u64 {
        self.cluster.AllocTSO()
    }

    /// CurrentTSO returns the latest allocated TSO.
    /// 读取当前已分配的最新 TSO，不推进时钟。
    pub fn CurrentTSO(&self) -> u64 {
        self.cluster.CurrentTSO()
    }

    /// RegionIDs returns all known region IDs in key order.
    /// 按 key 序返回全部 region ID，供扫描与 Scatter 遍历。
    pub fn RegionIDs(&self) -> Vec<u64> {
        self.cluster.RegionIDs()
    }

    /// RegionSnapshot gets one region snapshot.
    /// 单 region 快照；不存在时返回默认值与 false。
    pub fn RegionSnapshot(&self, regionID: u64) -> (RegionState, bool) {
        let (state, ok) = self.cluster.RegionSnapshot(regionID);
        if !ok {
            return (RegionState::default(), false);
        }
        (toRegionState(state), true)
    }

    /// RegionSnapshotsOnStore returns region snapshots hosted on the store.
    /// 指定 store 上托管的全部 region 快照；底层错误映射为本地 Error。
    pub fn RegionSnapshotsOnStore(&self, storeID: u64) -> Result<Vec<RegionState>> {
        let states = self
            .cluster
            .RegionSnapshotsOnStore(storeID)
            .map_err(|e| Error::new(e.to_string()))?;
        Ok(states.into_iter().map(toRegionState).collect())
    }

    /// GlobalCheckpoint returns the latest checkpoint uploaded by advancer.
    /// 读取 advancer 上传的最新全局 checkpoint（未上传则为 0）。
    pub fn GlobalCheckpoint(&self) -> u64 {
        self.state.lock().unwrap().global_checkpoint
    }

    /// 将 store 上各 region checkpoint 推进到给定值，供 FlushSim 调用。
    pub(crate) fn flushStore(
        &self,
        ctx: &Context,
        storeID: u64,
        checkpoint: u64,
    ) -> Result<Vec<RegionState>> {
        let (fc_ctx, cancel) = FcContext::with_cancel();
        if ctx.is_done() {
            cancel.cancel_with(
                ctx.err_message()
                    .unwrap_or_else(|| "context canceled".into()),
            );
        }

        let finished = Arc::new(AtomicBool::new(false));
        let finished_watch = Arc::clone(&finished);
        let ctx_watch = ctx.clone();
        let cancel_watch = cancel.clone();
        let cancellation_bridge = thread::spawn(move || {
            while !finished_watch.load(Ordering::SeqCst) {
                if ctx_watch.is_done() {
                    cancel_watch.cancel_with(
                        ctx_watch
                            .err_message()
                            .unwrap_or_else(|| "context canceled".into()),
                    );
                    return;
                }
                thread::sleep(Duration::from_millis(1));
            }
        });

        let states = self
            .cluster
            .ApplyCheckpointToStore(&fc_ctx, storeID, checkpoint)
            .map_err(|e| Error::new(e.to_string()));
        finished.store(true, Ordering::SeqCst);
        cancellation_bridge.join().unwrap();
        let states = states?;
        Ok(states.into_iter().map(toRegionState).collect())
    }

    /// Scatter reassigns regions to random stores. Regions moved to another store
    /// will have checkpoint reset to current global checkpoint.
    /// 随机重分配 region 所属 store；迁出后 checkpoint 重置为当前全局值并 bump epoch。
    pub fn Scatter(&self) -> Vec<u64> {
        let mut state = self.state.lock().unwrap();
        let storeIDs = self.cluster.StoreIDs();
        // 单 store 无法散射，直接返回空。
        if storeIDs.len() <= 1 {
            return Vec::new();
        }

        let global = state.global_checkpoint;
        let mut affected = Vec::new();
        for regionID in self.cluster.RegionIDs() {
            let (snap, ok) = self.cluster.RegionSnapshot(regionID);
            if !ok {
                continue;
            }
            let nextStoreID = storeIDs[state.rng.IntN(storeIDs.len())];
            // 抽到原 store 则跳过，避免无意义 epoch bump。
            if nextStoreID == snap.StoreID {
                continue;
            }
            self.cluster.TransferRegionTo(regionID, &[nextStoreID]);
            self.cluster.SetRegionLeader(regionID, nextStoreID);
            self.cluster.BumpRegionEpoch(regionID);
            // 迁出后进度回退到全局 checkpoint，模拟新 leader 冷启动。
            self.cluster.SetRegionCheckpoint(regionID, global);
            affected.push(regionID);
        }
        affected.sort();
        affected
    }

    /// 当前绑定的日志备份任务名。
    pub fn task_name(&self) -> String {
        self.state.lock().unwrap().task_name.clone()
    }

    /// 任务起始 TS（构造时 ComposeTS 的结果）。
    pub fn task_start(&self) -> u64 {
        self.state.lock().unwrap().task_start
    }

    /// 上传 V3 全局 checkpoint：校验任务名且禁止回滚，成功后唤醒等待者。
    pub(crate) fn upload_v3_global_checkpoint(
        &self,
        taskName: &str,
        checkpoint: u64,
    ) -> std::result::Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if taskName != state.task_name {
            return Err(format!("unknown task \"{taskName}\""));
        }
        // 全局 checkpoint 必须单调不减。
        if checkpoint < state.global_checkpoint {
            return Err(format!(
                "checkpoint rollback: {} -> {checkpoint}",
                state.global_checkpoint
            ));
        }
        state.global_checkpoint = checkpoint;
        state.checkpoint_gen += 1;
        self.checkpoint_cv.notify_all();
        Ok(())
    }

    /// 按任务名读取全局 checkpoint；任务不匹配则错误。
    pub(crate) fn get_global_checkpoint(&self, taskName: &str) -> std::result::Result<u64, String> {
        let state = self.state.lock().unwrap();
        if taskName != state.task_name {
            return Err(format!("unknown task \"{taskName}\""));
        }
        Ok(state.global_checkpoint)
    }

    /// 清空指定任务的全局 checkpoint（置 0），不递增世代。
    pub(crate) fn clear_v3_global_checkpoint(
        &self,
        taskName: &str,
    ) -> std::result::Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if taskName != state.task_name {
            return Err(format!("unknown task \"{taskName}\""));
        }
        state.global_checkpoint = 0;
        Ok(())
    }

    /// 阻塞直到全局 checkpoint > current，或 Context 取消。
    /// 用 Condvar 短超时轮询，避免永久挂起；世代变化也视为推进信号。
    pub(crate) fn wait_global_checkpoint_advance(
        &self,
        ctx: &Context,
        taskName: &str,
        current: u64,
    ) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.task_name != taskName {
            return Err(Error::new(format!(
                "task name mismatch: {taskName} and {}",
                state.task_name
            )));
        }
        // 已经领先则立即返回。
        if state.global_checkpoint > current {
            return Ok(());
        }
        let start_gen = state.checkpoint_gen;
        loop {
            if ctx.is_done() {
                return Err(Error::new(
                    ctx.err_message()
                        .unwrap_or_else(|| "context canceled".into()),
                ));
            }
            // 50ms 超时唤醒后复查取消与进度，对齐 Go select 轮询语义。
            let (guard, _result) = self
                .checkpoint_cv
                .wait_timeout(state, std::time::Duration::from_millis(50))
                .unwrap();
            state = guard;
            if state.checkpoint_gen > start_gen || state.global_checkpoint > current {
                return Ok(());
            }
        }
    }
}

/// fakecluster::RegionState → 本包 RegionState 字段映射。
fn toRegionState(r: fakecluster::RegionState) -> RegionState {
    RegionState {
        ID: r.ID,
        Epoch: r.Epoch,
        StoreID: r.StoreID,
        StartKey: r.StartKey,
        EndKey: r.EndKey,
        Checkpoint: r.Checkpoint,
    }
}
