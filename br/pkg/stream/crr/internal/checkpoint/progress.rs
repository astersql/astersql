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

//! CRR 检查点计算器的一轮推进与下游等待逻辑，对齐 Go `progress.go`。
//! 职责：拉取上游全局检查点、扫描并并发加载 meta、等待对象同步、推进 per-store 水位。
//! 约束：仅在全部存活 store 都有同步水位时才提升 `synced_ts`；缺失 store 会阻塞全局推进。
//! 事件：通过 `observe*` 向 StatusObserver 上报 Waiting/Planned/Advanced/Failed，供服务层指标与 HTTP 状态。
//! 数据流：PD checkpoint → meta 迭代 → load_meta_file → wait_object_sync → advance_synced_state → 返回安全点。
//! 与 calculator 主循环配合：本文件不拥有持久化，只突变内存 `state` 并观察事件。
//! 并发模型：meta 读取用有界线程池；下游轮询单线程，避免 Sync 实现的线程安全假设被放大。

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::calculator::{Calculator, CheckpointEvent, Context, Error, EventType, FileStatistic};
use crate::storage::{load_meta_file, parsedMetaFile};

/// 单轮扫描计划：待同步路径集合、各 store 本轮最大 flushTS、以及文件统计快照。
/// 对应 Go roundPlan；pending 去重后驱动 `wait_object_sync`。
pub(crate) struct roundPlan {
    /// meta 与 data file 路径集合（值为占位 unit）。
    pub pending_paths: HashMap<String, ()>,
    /// 本轮已加载 meta 中按 store 取 max(flush_ts)，用于推进 synced_by_store。
    pub max_flush_ts_by_store: HashMap<u64, u64>,
    /// 本轮统计：读 meta 数、跳过数、预估日志数、后缀分布等。
    pub statistic: FileStatistic,
}

/// 构造空 roundPlan；PlannedFileSuffixCounts 显式置空 Map，避免 Default 与 Go 侧 nil map 语义漂移。
fn new_round_plan() -> roundPlan {
    roundPlan {
        pending_paths: HashMap::new(),
        max_flush_ts_by_store: HashMap::new(),
        statistic: FileStatistic {
            PlannedFileSuffixCounts: HashMap::new(),
            ..Default::default()
        },
    }
}

impl roundPlan {
    /// 将已解析 meta 记入计划：meta 路径必入 pending；data file 去重新增才计入预估同步日志数。
    /// 同时更新该 store 的 max flush_ts，供本轮结束后推进水位。
    fn record_loaded_meta(&mut self, loaded_meta: crate::storage::loadedMetaFile) {
        // 每成功解析一份 meta 计一次读次数，与是否跳过 data 去重无关。
        self.statistic.UpstreamReadMetaFileCount += 1;
        self.record_pending_path(loaded_meta.path.clone());
        for log_path in loaded_meta.data_file_paths {
            // 路径已存在则不计 EstimatedSyncLogFileCount，避免重复引用夸大待同步量。
            if self.record_pending_path(log_path) {
                self.statistic.EstimatedSyncLogFileCount += 1;
            }
        }
        let entry = self
            .max_flush_ts_by_store
            .entry(loaded_meta.store_id)
            .or_insert(0);
        // 同 store 多份 meta 时取最大 flush_ts，与 Go 侧 max 语义一致。
        if loaded_meta.flush_ts > *entry {
            *entry = loaded_meta.flush_ts;
        }
    }

    /// 登记待同步路径；首次出现返回 true，并按后缀累计 PlannedFileSuffixCounts。
    fn record_pending_path(&mut self, file_path: String) -> bool {
        if self.pending_paths.contains_key(&file_path) {
            return false;
        }
        self.pending_paths.insert(file_path.clone(), ());
        let suffix = path_suffix(&file_path);
        *self
            .statistic
            .PlannedFileSuffixCounts
            .entry(suffix)
            .or_insert(0) += 1;
        true
    }
}

/// 有界并发信号量：限制 meta 并发读取，对齐 Go `MetaReadConcurrency`。
/// 用 Mutex+Condvar 实现，避免在 thread::scope 内引入额外异步运行时。
struct ConcurrencyLimiter {
    /// 剩余可发放许可数。
    available: Mutex<usize>,
    /// 许可耗尽时的等待队列。
    cvar: Condvar,
}

impl ConcurrencyLimiter {
    /// `limit` 应为至少 1；调用方对 cfg 做 max(1)。
    fn new(limit: usize) -> Self {
        Self {
            available: Mutex::new(limit),
            cvar: Condvar::new(),
        }
    }

    /// 阻塞直到取得一个许可；返回的 Guard 在 Drop 时归还并唤醒等待者。
    fn acquire(&self) -> ConcurrencyGuard<'_> {
        let mut available = self.available.lock().unwrap();
        while *available == 0 {
            available = self.cvar.wait(available).unwrap();
        }
        *available -= 1;
        ConcurrencyGuard { limiter: self }
    }
}

/// RAII 许可：离开作用域即释放并发槽位。
struct ConcurrencyGuard<'a> {
    limiter: &'a ConcurrencyLimiter,
}

impl Drop for ConcurrencyGuard<'_> {
    fn drop(&mut self) {
        let mut available = self.limiter.available.lock().unwrap();
        *available += 1;
        // 一次只唤醒一个等待者，匹配单许可释放语义。
        self.limiter.cvar.notify_one();
    }
}

impl Calculator {
    // —— 以下方法挂在 Calculator 上，供 ComputeNextCheckpoint 主循环调用 ——
    /// 轮询 PD 全局检查点；仅当严格大于 `last_checkpoint` 时返回 advanced=true。
    /// 未前进时发 EventWaitingUpstream，供服务层展示“等待上游”相位。
    pub(crate) fn poll_upstream_checkpoint(&mut self, ctx: &Context) -> Result<(u64, bool), Error> {
        let checkpoint = self
            .deps
            .PD
            .GetGlobalCheckpointForTask(ctx, &self.cfg.TaskName)
            .map_err(|err| {
                Error::new(format!(
                    "get global checkpoint for task {}: {}",
                    self.cfg.TaskName, err
                ))
            })?;
        // 严格大于：等于 last_checkpoint 视为未前进，进入 WaitingUpstream。
        if checkpoint > self.state.last_checkpoint {
            self.observe(CheckpointEvent {
                Type: EventType::EventUpstreamAdvanced,
                TaskName: self.cfg.TaskName.clone(),
                UpstreamCheckpoint: checkpoint,
                ..Default::default()
            });
            return Ok((checkpoint, true));
        }
        // LoopIteration 固定为 1：此处只做一次探测，真正的等待在服务层 Waiter。
        self.observe(CheckpointEvent {
            Type: EventType::EventWaitingUpstream,
            TaskName: self.cfg.TaskName.clone(),
            LoopIteration: 1,
            UpstreamCheckpoint: checkpoint,
            ..Default::default()
        });
        Ok((checkpoint, false))
    }

    /// 从 PD 加载存活 store 集合；过滤 ID==0 的占位项，与 Go Stores 过滤一致。
    pub(crate) fn load_alive_stores(&self, ctx: &Context) -> Result<HashMap<u64, ()>, Error> {
        let stores = self
            .deps
            .PD
            .Stores(ctx)
            .map_err(|err| Error::new(format!("load alive stores from pd: {err}")))?;
        // 用 HashMap<(),()> 模拟 Go map[uint64]struct{} 集合语义。
        let mut alive_stores = HashMap::with_capacity(stores.len());
        for store in stores {
            // ID 0 在 PD mock/占位里可能出现，不得参与缺失 store 判定。
            if store.ID == 0 {
                continue;
            }
            alive_stores.insert(store.ID, ());
        }
        Ok(alive_stores)
    }

    /// 规划本轮：先过滤已按 store 同步过的 meta，再按 MetaReadConcurrency 并发读内容。
    /// 取消上下文或任一加载失败会中止；首错保留，后续错误丢弃（对齐 Go 首错语义）。
    pub(crate) fn plan_round(&self, ctx: &Context) -> Result<roundPlan, Error> {
        // plan 用 Mutex 便于并发加载线程写入；顺序阶段只单线程持锁。
        let plan = Mutex::new(new_round_plan());
        // 首个读取错误必须取消同轮兄弟任务，对齐 Go errgroup.WithContext。
        let (plan_ctx, cancel_plan) = Context::WithCancel(ctx);
        let cancel_plan = Arc::new(cancel_plan);
        let mut iter_err: Option<Error> = None;
        // 先收集待加载列表，再开线程，避免迭代器与并发借用冲突。
        let mut to_load: Vec<parsedMetaFile> = Vec::new();

        for item in self.new_meta_file_iter(&plan_ctx) {
            match item {
                Err(err) => {
                    // 迭代错误立即中断收集；不与后续加载错误合并。
                    iter_err = Some(err);
                    break;
                }
                Ok(meta_file) => {
                    // flush_ts 未超过该 store 已同步水位则跳过，避免重复等待旧文件。
                    if let Some(synced_ts) = self.state.synced_by_store.get(&meta_file.store_id) {
                        if meta_file.flush_ts <= *synced_ts {
                            plan.lock()
                                .unwrap()
                                .statistic
                                .SkippedStoreSyncedMetaFileCount += 1;
                            continue;
                        }
                    }
                    to_load.push(meta_file);
                }
            }
        }

        if let Some(err) = iter_err {
            cancel_plan();
            return Err(err);
        }

        // Go 侧同样把并发下限钳到 1，防止配置 0 导致无工人。
        let concurrency = self.cfg.MetaReadConcurrency.max(1) as usize;
        let limiter = ConcurrencyLimiter::new(concurrency);
        // 多线程首错写入；成功路径不清理，由 scope 结束后统一检查。
        let load_err = Mutex::new(None::<Error>);

        // thread::scope 保证工作线程在返回前结束，可安全借用 Upstream/plan。
        thread::scope(|scope| {
            for meta_file in to_load {
                if plan_ctx.Done() {
                    let mut slot = load_err.lock().unwrap();
                    // 仅记录首个取消错误，避免覆盖更早的加载失败。
                    if slot.is_none() {
                        *slot = plan_ctx.Err();
                    }
                    break;
                }
                // 许可在 spawn 前取得：限制“飞行中”任务数，而非已排队任务数。
                let guard = limiter.acquire();
                let ctx = plan_ctx.clone();
                let upstream = &*self.deps.Upstream;
                let plan = &plan;
                let load_err = &load_err;
                let cancel_plan = Arc::clone(&cancel_plan);
                scope.spawn(move || {
                    // 许可必须持有到读取任务结束；若留在 spawn 调用方，
                    // 它会在启动线程后立即释放，导致 MetaReadConcurrency 形同虚设。
                    let _guard = guard;
                    // 闭包内再包一层 Result，统一把 load/parse 错误写入首错槽。
                    if let Err(err) = (|| {
                        let loaded = load_meta_file(&ctx, upstream, meta_file)?;
                        plan.lock().unwrap().record_loaded_meta(loaded);
                        Ok::<(), Error>(())
                    })() {
                        let mut slot = load_err.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(err);
                            cancel_plan();
                        }
                    }
                });
            }
        });

        if let Some(err) = load_err.into_inner().unwrap() {
            return Err(err);
        }
        Ok(plan.into_inner().unwrap())
    }

    /// 轮询下游同步状态直至 pending 清空；每轮间隔 PollInterval，可被 ctx 取消。
    /// 未完成时上报 EventWaitingDownstream，并累计 DownstreamCheckFileCount。
    pub(crate) fn wait_object_sync(
        &self,
        ctx: &Context,
        pending_paths: &mut HashMap<String, ()>,
        statistic: &mut FileStatistic,
    ) -> Result<(), Error> {
        let mut loop_iteration = 0u64;
        while !pending_paths.is_empty() {
            // 每轮克隆键列表，允许循环中 remove；顺序不保证，与 Go map 迭代一致。
            let paths: Vec<String> = pending_paths.keys().cloned().collect();
            for file_path in paths {
                // 无论是否已同步都计数检查次数，便于观察下游滞后。
                statistic.record_downstream_check(&file_path);
                let exists = self.deps.Sync.FileSynced(ctx, &file_path).map_err(|err| {
                    Error::new(format!("check sync status for {file_path}: {err}"))
                })?;
                if exists {
                    pending_paths.remove(&file_path);
                }
            }
            if pending_paths.is_empty() {
                return Ok(());
            }
            // 迭代从 1 起算，对应“已完成至少一次完整扫描仍未清空”。
            loop_iteration += 1;
            self.observe_waiting_downstream(loop_iteration, pending_paths.len() as i32, statistic);
            sleep_with_context(ctx, self.cfg.PollInterval)?;
        }
        Ok(())
    }

    /// 用本轮 max_flush_ts 抬升 synced_by_store，再按存活 store 裁剪离线项。
    /// 全局 synced_ts 取裁剪前各 store 水位的最小值；有缺失 store 则不前进。
    pub(crate) fn advance_synced_state(
        &mut self,
        alive_stores: &HashMap<u64, ()>,
        max_flush_ts_by_store: &HashMap<u64, u64>,
    ) {
        // 先合并本轮水位；即使后续因缺失 store 不抬 synced_ts，per-store 进度仍保留。
        for (store_id, flush_ts) in max_flush_ts_by_store {
            let entry = self.state.synced_by_store.entry(*store_id).or_insert(0);
            if *flush_ts > *entry {
                *entry = *flush_ts;
            }
        }

        // 先克隆再 prune：min 候选必须包含本轮刚更新、即将被裁掉的离线 store 水位。
        let synced_by_store_before_prune = self.state.synced_by_store.clone();
        self.state
            .synced_by_store
            .retain(|store_id, _| alive_stores.contains_key(store_id));

        if !self.check_missing_store(alive_stores) {
            return;
        }

        let Some(synced_candidate) = synced_by_store_before_prune.values().min().copied() else {
            return;
        };
        // 只允许单调前进，防止并发/重启路径把水位回拨。
        if synced_candidate > self.state.synced_ts {
            self.state.synced_ts = synced_candidate;
        }
    }

    /// 若任一存活 store 尚无 synced_by_store 条目则返回 false，阻止全局水位提升。
    /// 排序 missing 仅为与 Go 日志顺序稳定一致；此处暂未输出日志。
    pub(crate) fn check_missing_store(&self, alive_stores: &HashMap<u64, ()>) -> bool {
        let mut missing_stores: Vec<u64> = alive_stores
            .keys()
            .filter(|store_id| !self.state.synced_by_store.contains_key(store_id))
            .copied()
            .collect();
        if missing_stores.is_empty() {
            return true;
        }
        missing_stores.sort_unstable();
        let _ = missing_stores;
        false
    }

    /// 下游等待循环的观察点：携带迭代次数、剩余文件数与统计快照。
    fn observe_waiting_downstream(
        &self,
        loop_iteration: u64,
        pending_file_count: i32,
        statistic: &FileStatistic,
    ) {
        self.observe(CheckpointEvent {
            Type: EventType::EventWaitingDownstream,
            TaskName: self.cfg.TaskName.clone(),
            LoopIteration: loop_iteration,
            PendingFileCount: pending_file_count,
            Statistic: Some(statistic.snapshot()),
            ..Default::default()
        });
    }
}

/// 本轮计划完成后上报 EventRoundPlanned，并返回不可变统计快照供后续等待/推进复用。
pub(crate) fn observe_round_planned(
    calc: &Calculator,
    upstream_checkpoint: u64,
    alive_stores: &HashMap<u64, ()>,
    round: &roundPlan,
) -> FileStatistic {
    // snapshot 断开与 round 可变统计的共享，防止等待阶段继续 mutate 影响已上报值。
    let statistic = round.statistic.snapshot();
    calc.observe(CheckpointEvent {
        Type: EventType::EventRoundPlanned,
        TaskName: calc.cfg.TaskName.clone(),
        UpstreamCheckpoint: upstream_checkpoint,
        AliveStoreCount: alive_stores.len() as i32,
        PendingFileCount: round.pending_paths.len() as i32,
        Statistic: Some(statistic.clone()),
        ..Default::default()
    });
    statistic
}

/// 下游确认同步且水位推进后上报；携带 SyncedTS/SyncedByStore 供 resume 与指标写入。
pub(crate) fn observe_checkpoint_advanced(
    calc: &Calculator,
    upstream_checkpoint: u64,
    alive_stores: &HashMap<u64, ()>,
    statistic: Option<FileStatistic>,
) {
    calc.observe(CheckpointEvent {
        Type: EventType::EventCheckpointAdvanced,
        TaskName: calc.cfg.TaskName.clone(),
        UpstreamCheckpoint: upstream_checkpoint,
        SyncedTS: calc.state.synced_ts,
        SyncedByStore: calc.state.synced_by_store.clone(),
        SyncedByStoreSet: true,
        AliveStoreCount: alive_stores.len() as i32,
        Statistic: statistic,
        ..Default::default()
    });
}

/// 计算失败路径：把错误与可选统计挂到 EventCalculationFailed，触发服务降级状态。
pub(crate) fn observe_calculation_failed(
    calc: &Calculator,
    err: Error,
    statistic: Option<FileStatistic>,
) {
    calc.observe(CheckpointEvent {
        Type: EventType::EventCalculationFailed,
        TaskName: calc.cfg.TaskName.clone(),
        Statistic: statistic,
        Err: Some(err),
        ..Default::default()
    });
}

/// 可取消睡眠：每 10ms 检查 ctx，避免长 PollInterval 下取消延迟过大。
fn sleep_with_context(ctx: &Context, d: Duration) -> Result<(), Error> {
    // 10ms 粒度与 Go context sleep helper 同量级，兼顾取消响应与调度开销。
    let step = Duration::from_millis(10);
    let mut remaining = d;
    while remaining > Duration::ZERO {
        if let Some(err) = ctx.Err() {
            return Err(err);
        }
        let sleep_for = step.min(remaining);
        thread::sleep(sleep_for);
        remaining = remaining.saturating_sub(sleep_for);
    }
    // 睡眠结束后再查一次，覆盖恰好在最后一步被取消的竞态。
    ctx.Err().map_or(Ok(()), Err)
}

/// 提取文件后缀用于指标分桶；无后缀为 `<none>`，过长后缀归为 `<other>`（长度阈值 5）。
/// 与 Go pathSuffix 一致，避免高基数扩展名污染 PlannedFileSuffixCounts。
pub(crate) fn path_suffix(file_path: &str) -> String {
    // 只看 basename，避免目录段中的点干扰后缀识别。
    let base = file_path.rsplit('/').next().unwrap_or(file_path);
    let suffix = base
        .rsplit_once('.')
        .map(|(_, ext)| format!(".{ext}"))
        .unwrap_or_default();
    if suffix.is_empty() {
        return "<none>".to_string();
    }
    // 含点号本身：`.meta` 长度为 5，刚好保留；更长归桶。
    if suffix.len() > 5 {
        return "<other>".to_string();
    }
    suffix
}
