// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 日志备份检查点推进器（Checkpoint Advancer）。
//! 负责绑定任务范围、轮询 region flush TS、合并跨 store 最小检查点，
//! 并上传 V3 全局检查点；与 Go `advancer.go` 语义对齐。
//! 锁解决路径在 ScanLock locked 时会二分降低 maxVersion 重试。
//! Owner 侧通过 tick/optionalTick/importantTick 分层推进；暂停时整轮跳过。
//! 配置可来自命令行或 TiDB 嵌入默认值，运行时可被 flush 间隔覆盖。
//! `checkpoints` 树的 MinValue 即跨范围全局最小 flush TS，供上传使用。
//! resolve-lock 辅助函数与 Go 同名符号保持相同的 TSO 算术与重试上限。

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use astersql_br_pkg_streamhelper_config::{
    CommandConfig, Config, DefaultCommandConfig, DefaultTiDBConfig, TiDBConfig,
};
use astersql_br_pkg_streamhelper_spans::{
    Full, NewFullWith, Sorted, Span, ValueSortedFull, Valued,
};

use crate::advancer_cliext::{EventType, TaskEvent};
use crate::advancer_env::Env;
use crate::collector::{NewClusterCollector, OnSuccessHook};
use crate::flush_subscriber::FlushSubscriber;
use crate::regioniter::IterateRegion;
use crate::stubs::{KeyRange, StreamBackupTaskInfo};

/// 外部存储全局检查点键前缀（历史路径残留常量）。
pub const streamBackupGlobalCheckpointPrefix: &str = "v1/global_checkpoint";
/// ScanLock locked 时最多额外重试次数。
pub const resolveLockMaxVersionMaxRetry: i32 = 2;
/// 重试下界相对检查点的滞后（物理时间 10s，写入 TSO 高位）。
pub const resolveLockRetryLowerBoundLag: Duration = Duration::from_secs(10);
/// 周期性刷新 TiKV log-backup 配置的间隔。
pub const logBackupConfigRefreshInterval: Duration = Duration::from_secs(60);
/// 拉取单 Store 配置的超时。
pub const logBackupConfigFetchTimeout: Duration = Duration::from_secs(10);

/// Physical time unit in TSO (same as oracle.GetPhysical).
/// TSO 物理时间左移位数，与 oracle.GetPhysical 一致。
const PHYSICAL_SHIFT_BITS: u64 = 18;

/// 在 TSO `ts` 上增加物理时长 `d`（饱和加法避免溢出）。
fn tso_after(ts: u64, d: Duration) -> u64 {
    let ms = d.as_millis() as u64;
    ts.saturating_add(ms << PHYSICAL_SHIFT_BITS)
}

/// 从当前 TSO 回退物理时长 `d`，用于 resolve-lock 上界计算。
fn tso_before_from_ts(current: u64, d: Duration) -> u64 {
    let ms = d.as_millis() as u64;
    current.saturating_sub(ms << PHYSICAL_SHIFT_BITS)
}

/// 某一 key 范围上的检查点快照；含上次 resolve-lock 时刻。
#[derive(Clone, Debug)]
pub struct Checkpoint {
    /// 范围起点（含）。
    pub StartKey: Vec<u8>,
    /// 范围终点（不含）。
    pub EndKey: Vec<u8>,
    /// 检查点 TSO。
    pub TS: u64,
    /// 上次对该范围执行 resolve-lock 的本地时间。
    pub resolveLockTime: Instant,
}

/// 仅带 TS 的空范围检查点，用于测试与初始化。
pub fn newCheckpointWithTS(ts: u64) -> Checkpoint {
    Checkpoint {
        StartKey: Vec::new(),
        EndKey: Vec::new(),
        TS: ts,
        resolveLockTime: Instant::now(),
    }
}

/// 从有值 Span 构造检查点。
pub fn newCheckpointWithSpan(s: Valued) -> Checkpoint {
    Checkpoint {
        StartKey: s.Key.StartKey,
        EndKey: s.Key.EndKey,
        TS: s.Value,
        resolveLockTime: Instant::now(),
    }
}

impl Checkpoint {
    /// 安全读点：TS>0 时返回 TS-1，供 GC 阻塞使用。
    pub fn safeTS(&self) -> u64 {
        if self.TS == 0 { 0 } else { self.TS - 1 }
    }

    /// 比较范围与 TS 是否完全一致（忽略 resolveLockTime）。
    pub fn equal(&self, o: &Checkpoint) -> bool {
        self.StartKey == o.StartKey && self.EndKey == o.EndKey && self.TS == o.TS
    }

    /// 距上次 resolve-lock 是否已超过配置间隔。
    pub fn needResolveLocks(&self, interval: Duration) -> bool {
        self.resolveLockTime.elapsed() > interval
    }
}

/// 内部配置包装：命令行模式或 TiDB 嵌入模式。
enum Cfg {
    Command(CommandConfig),
    TiDB(TiDBConfig),
}

impl Config for Cfg {
    /// 失败退避时长。
    fn GetBackoffTime(&self) -> Duration {
        match self {
            Cfg::Command(c) => c.GetBackoffTime(),
            Cfg::TiDB(c) => c.GetBackoffTime(),
        }
    }
    /// 单次 tick 超时。
    fn TickTimeout(&self) -> Duration {
        match self {
            Cfg::Command(c) => c.TickTimeout(),
            Cfg::TiDB(c) => c.TickTimeout(),
        }
    }
    /// 正常路径开始轮询阈值。
    fn GetDefaultStartPollThreshold(&self) -> Duration {
        match self {
            Cfg::Command(c) => c.GetDefaultStartPollThreshold(),
            Cfg::TiDB(c) => c.GetDefaultStartPollThreshold(),
        }
    }
    /// 订阅错误后的轮询阈值。
    fn GetSubscriberErrorStartPollThreshold(&self) -> Duration {
        match self {
            Cfg::Command(c) => c.GetSubscriberErrorStartPollThreshold(),
            Cfg::TiDB(c) => c.GetSubscriberErrorStartPollThreshold(),
        }
    }
    /// resolve-lock 周期间隔。
    fn GetResolveLockInterval(&self) -> Duration {
        match self {
            Cfg::Command(c) => c.GetResolveLockInterval(),
            Cfg::TiDB(c) => c.GetResolveLockInterval(),
        }
    }
    /// 检查点滞后上限；超过可触发暂停策略。
    fn GetCheckPointLagLimit(&self) -> Duration {
        match self {
            Cfg::Command(c) => c.GetCheckPointLagLimit(),
            Cfg::TiDB(c) => c.GetCheckPointLagLimit(),
        }
    }
}

/// 检查点推进器核心状态：任务绑定、区间检查点树、订阅与暂停标志。
/// `checkpoints` 为按 key 排序的有值区间，MinValue 即全局最小检查点。
pub struct CheckpointAdvancer {
    /// 集群/元数据/锁解决环境。
    env: Arc<dyn Env>,
    /// 当前绑定的流备份任务；None 表示无任务。
    task: Mutex<Option<StreamBackupTaskInfo>>,
    /// 任务覆盖的 key 范围列表。
    taskRange: Mutex<Vec<KeyRange>>,
    /// 上次写入外部存储的检查点（纳秒原子；本端口可能未启用存储）。
    lastExternalStorageCheckpoint: AtomicI64,
    /// 运行时配置（Command 或 TiDB）。
    cfg: Mutex<Cfg>,
    /// 覆盖配置的 resolve-lock 间隔（纳秒）；0 表示走 cfg。
    resolveLockInterval: AtomicI64,
    /// try-advance 轮询阈值（纳秒）；可由 flush 间隔推导。
    tryAdvanceThreshold: AtomicI64,
    /// 最近一次重要检查点快照。
    lastCheckpoint: Mutex<Option<Checkpoint>>,
    /// 是否正在 resolve-lock，防止重入。
    inResolvingLock: AtomicBool,
    /// 暂停时 tick 直接返回，不推进。
    isPaused: AtomicBool,
    /// 区间检查点树；SetTask 时初始化。
    checkpoints: Arc<Mutex<Option<ValueSortedFull>>>,
    /// flush 订阅器；Owner 期间持有。
    subscriber: Mutex<Option<FlushSubscriber>>,
    #[cfg(test)]
    subscribeTickHook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

/// 以 TiDB 默认配置创建推进器。
pub fn NewTiDBCheckpointAdvancer(env: Arc<dyn Env>) -> CheckpointAdvancer {
    CheckpointAdvancer {
        env,
        task: Mutex::new(None),
        taskRange: Mutex::new(Vec::new()),
        lastExternalStorageCheckpoint: AtomicI64::new(0),
        cfg: Mutex::new(Cfg::TiDB(DefaultTiDBConfig())),
        resolveLockInterval: AtomicI64::new(0),
        tryAdvanceThreshold: AtomicI64::new(0),
        lastCheckpoint: Mutex::new(None),
        inResolvingLock: AtomicBool::new(false),
        isPaused: AtomicBool::new(false),
        checkpoints: Arc::new(Mutex::new(None)),
        subscriber: Mutex::new(None),
        #[cfg(test)]
        subscribeTickHook: Mutex::new(None),
    }
}

/// 以命令行默认配置创建推进器。
pub fn NewCommandCheckpointAdvancer(env: Arc<dyn Env>) -> CheckpointAdvancer {
    CheckpointAdvancer {
        env,
        task: Mutex::new(None),
        taskRange: Mutex::new(Vec::new()),
        lastExternalStorageCheckpoint: AtomicI64::new(0),
        cfg: Mutex::new(Cfg::Command(DefaultCommandConfig())),
        resolveLockInterval: AtomicI64::new(0),
        tryAdvanceThreshold: AtomicI64::new(0),
        lastCheckpoint: Mutex::new(None),
        inResolvingLock: AtomicBool::new(false),
        isPaused: AtomicBool::new(false),
        checkpoints: Arc::new(Mutex::new(None)),
        subscriber: Mutex::new(None),
        #[cfg(test)]
        subscribeTickHook: Mutex::new(None),
    }
}

/// 默认构造：Rust 端口固定选 TiDB 配置（Go export_test 会随机二选一）。
pub fn NewCheckpointAdvancer(env: Arc<dyn Env>) -> CheckpointAdvancer {
    // export_test randomly chooses; pick TiDB deterministically for Rust.
    NewTiDBCheckpointAdvancer(env)
}

impl CheckpointAdvancer {
    /// Apply one metadata-listener event using the same state transitions as Go
    /// `onTaskEvent`. External cleanup failures are deliberately best-effort.
    pub(crate) fn onTaskEvent(&self, event: TaskEvent) -> Result<(), String> {
        match event.Type {
            EventType::EventAdd => {
                let info = event
                    .Info
                    .ok_or_else(|| format!("task {} add event has no task info", event.Name))?;
                let checkpoint = self
                    .env
                    .GetGlobalCheckpointForTask(&event.Name)
                    .unwrap_or(0)
                    .max(info.StartTs);
                self.SetTask(info, event.Ranges);
                self.UpdateLastCheckpoint(newCheckpointWithTS(checkpoint));
                let _ = self.env.BlockGCUntil(checkpoint.saturating_sub(1));
            }
            EventType::EventDel => {
                self.closeGlobalCheckpointStorage();
                *self.task.lock().unwrap() = None;
                self.isPaused.store(false, Ordering::SeqCst);
                self.taskRange.lock().unwrap().clear();
                self.stopSubscriber();
                *self.checkpoints.lock().unwrap() = None;
                let _ = self.env.ClearV3GlobalCheckpointForTask(&event.Name);
                let _ = self.env.UnblockGC();
            }
            EventType::EventPause => {
                let matches = self
                    .task
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|task| task.Name == event.Name)
                    .unwrap_or(false);
                if matches {
                    self.isPaused.store(true, Ordering::SeqCst);
                }
            }
            EventType::EventResume => {
                let matches = self
                    .task
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|task| task.Name == event.Name)
                    .unwrap_or(false);
                if matches {
                    self.isPaused.store(false, Ordering::SeqCst);
                }
            }
            EventType::EventErr => {
                return Err(event.Err.unwrap_or_else(|| "task listener error".into()));
            }
        }
        Ok(())
    }

    /// Consume the initial task-listener snapshot. Every event is applied in
    /// delivery order; Env implementations may bridge their live watch here.
    pub(crate) fn StartTaskListener(&self) -> Result<(), String> {
        let mut events = Vec::new();
        self.env.Begin(&mut events)?;
        for event in events {
            self.onTaskEvent(event)?;
        }
        Ok(())
    }

    /// 是否已绑定任务。
    pub fn HasTask(&self) -> bool {
        self.task.lock().unwrap().is_some()
    }

    /// 是否仍持有 flush 订阅。
    pub fn HasSubscriptions(&self) -> bool {
        self.subscriber
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.SubscriptionCount() > 0)
            .unwrap_or(false)
    }

    /// 热更新命令行配置。
    pub fn UpdateConfigCommand(&self, newConf: CommandConfig) {
        *self.cfg.lock().unwrap() = Cfg::Command(newConf);
    }

    /// 热更新 TiDB 侧配置。
    pub fn UpdateConfigTiDB(&self, newConf: TiDBConfig) {
        *self.cfg.lock().unwrap() = Cfg::TiDB(newConf);
    }

    /// Mutate the command portion of the active configuration in place.
    ///
    /// Go's test export applies updates to the current concrete config, including
    /// the embedded `CommandConfig` used by TiDB mode. Keeping this operation here
    /// preserves the private enum variant instead of rebuilding defaults.
    pub(crate) fn updateConfigWithForTest(&self, f: impl FnOnce(&mut CommandConfig)) {
        let mut cfg = self.cfg.lock().unwrap();
        match &mut *cfg {
            Cfg::Command(command) => f(command),
            Cfg::TiDB(tidb) => f(&mut tidb.CommandConfig),
        }
    }

    /// 解析 resolve-lock 间隔：原子覆盖优先，否则读 cfg。
    pub fn getResolveLockInterval(&self) -> Duration {
        let loaded = self.resolveLockInterval.load(Ordering::SeqCst);
        if loaded > 0 {
            Duration::from_nanos(loaded as u64)
        } else {
            self.cfg.lock().unwrap().GetResolveLockInterval()
        }
    }

    /// 默认开始轮询阈值；可由 flush 间隔的 4/3 覆盖。
    pub fn getDefaultStartPollThreshold(&self) -> Duration {
        let loaded = self.tryAdvanceThreshold.load(Ordering::SeqCst);
        if loaded > 0 {
            Duration::from_nanos(loaded as u64)
        } else {
            self.cfg.lock().unwrap().GetDefaultStartPollThreshold()
        }
    }

    /// 订阅出错后的轮询阈值；覆盖值为 tryAdvance 的 9/20。
    pub fn getSubscriberErrorStartPollThreshold(&self) -> Duration {
        let loaded = self.tryAdvanceThreshold.load(Ordering::SeqCst);
        if loaded > 0 {
            Duration::from_nanos((loaded as u64) * 9 / 20)
        } else {
            self.cfg
                .lock()
                .unwrap()
                .GetSubscriberErrorStartPollThreshold()
        }
    }

    /// 记录最近一次检查点快照。
    pub fn UpdateLastCheckpoint(&self, p: Checkpoint) {
        *self.lastCheckpoint.lock().unwrap() = Some(p);
    }

    /// 单调更新最近检查点；范围或 TS 未变化时不刷新 resolve-lock 计时。
    fn setCheckpoint(&self, p: Checkpoint) -> bool {
        let mut last = self.lastCheckpoint.lock().unwrap();
        if let Some(old) = last.as_ref() {
            if p.TS < old.TS || p.equal(old) {
                return false;
            }
        }
        *last = Some(p);
        true
    }

    /// 按 PD TSO 的物理时间部分判断已上传检查点是否超过允许滞后。
    fn isCheckpointLagged(&self, task: &StreamBackupTaskInfo) -> Result<bool, String> {
        let limit = self.cfg.lock().unwrap().GetCheckPointLagLimit();
        if limit.is_zero() {
            return Ok(false);
        }
        let global_ts = self.env.GetGlobalCheckpointForTask(&task.Name)?;
        if global_ts < task.StartTs {
            return Ok(false);
        }
        let current_ts = self.env.FetchCurrentTS()?;
        let lag_tso = current_ts.saturating_sub(global_ts);
        let limit_tso = (limit.as_millis() as u64).saturating_mul(1 << PHYSICAL_SHIFT_BITS);
        Ok(lag_tso > limit_tso)
    }

    /// 查询是否处于 resolve-lock 临界区。
    pub fn GetInResolvingLock(&self) -> bool {
        self.inResolvingLock.load(Ordering::SeqCst)
    }

    /// 绑定任务与范围，并初始化区间检查点树（空范围则 Full）。
    pub fn SetTask(&self, info: StreamBackupTaskInfo, ranges: Vec<KeyRange>) {
        *self.task.lock().unwrap() = Some(info);
        *self.taskRange.lock().unwrap() = ranges;
        let spans: Vec<Span> = self
            .taskRange
            .lock()
            .unwrap()
            .iter()
            .map(|r| Span {
                StartKey: r.StartKey.clone(),
                EndKey: r.EndKey.clone(),
            })
            .collect();
        // 无显式范围时覆盖全键空间，与 Go Full() 一致。
        let init = if spans.is_empty() { Full() } else { spans };
        *self.checkpoints.lock().unwrap() = Some(Sorted(NewFullWith(&init, 0)));
    }

    /// 设置暂停标志；暂停时 tick 跳过推进。
    pub fn SetPaused(&self, paused: bool) {
        self.isPaused.store(paused, Ordering::SeqCst);
    }

    /// 从 Env 刷新 flush 间隔，并推导 resolve-lock / try-advance 阈值。
    pub fn refreshLogBackupFlushInterval(&self) -> Result<(), String> {
        let flush = self.env.GetLogBackupFlushInterval()?;
        if flush.is_zero() {
            return Ok(());
        }
        self.resolveLockInterval
            .store(flush.as_nanos() as i64, Ordering::SeqCst);
        // Go: tryAdvanceThreshold = flush * 4 / 3
        let try_advance = flush * 4 / 3;
        self.tryAdvanceThreshold
            .store(try_advance.as_nanos() as i64, Ordering::SeqCst);
        Ok(())
    }

    /// 扫描 `[start,end)` 内 Region，将结果送入 collector。
    pub fn GetCheckpointInRange(
        &self,
        start: &[u8],
        end: &[u8],
        collector: &mut crate::collector::ClusterCollector,
    ) -> Result<(), String> {
        let mut iter = IterateRegion(self.env.as_ref(), start, end);
        while !iter.Done() {
            let rs = iter.Next()?;
            for r in rs {
                collector.CollectRegion(r)?;
            }
        }
        Ok(())
    }

    /// 在持锁下访问区间检查点树；未初始化则返回 None。
    pub fn WithCheckpoints<R>(&self, f: impl FnOnce(&mut ValueSortedFull) -> R) -> Option<R> {
        let mut g = self.checkpoints.lock().unwrap();
        g.as_mut().map(f)
    }

    /// 折叠范围后按 Region 采集 flush TS，可选成功钩子合并进检查点树。
    pub fn tryAdvance(
        &self,
        ranges: &[KeyRange],
        on_success: Option<OnSuccessHook>,
    ) -> Result<crate::collector::StoreCheckpoints, String> {
        // 合并重叠区间，减少重复 Region 扫描。
        let collapsed = astersql_br_pkg_streamhelper_spans::Collapse(
            &ranges
                .iter()
                .map(|r| Span {
                    StartKey: r.StartKey.clone(),
                    EndKey: r.EndKey.clone(),
                })
                .collect::<Vec<_>>(),
        );
        let mut collector = NewClusterCollector(self.env.clone());
        if let Some(hook) = on_success {
            let checkpoints = self.checkpoints.clone();
            // 包装钩子：先 Merge 到本地树，再调用外部钩子。
            let wrapped: OnSuccessHook = Arc::new(move |ts, kr| {
                if let Ok(mut g) = checkpoints.lock() {
                    if let Some(cp) = g.as_mut() {
                        cp.Merge(Valued {
                            Key: Span {
                                StartKey: kr.StartKey.clone(),
                                EndKey: kr.EndKey.clone(),
                            },
                            Value: ts,
                        });
                    }
                }
                hook(ts, kr);
            });
            collector.SetOnSuccessHook(wrapped);
        }
        for span in collapsed {
            self.GetCheckpointInRange(&span.StartKey, &span.EndKey, &mut collector)?;
        }
        collector.Finish()
    }

    /// 重要 tick：取检查点树最小值并上传为任务全局检查点。
    pub fn importantTick(&self) -> Result<(), String> {
        let task = self.task.lock().unwrap().clone();
        let Some(task) = task else {
            return Ok(());
        };
        let min_ts = self
            .WithCheckpoints(|vsf| vsf.MinValue())
            .and_then(|v| v)
            .unwrap_or(0);
        if min_ts == 0 {
            return Ok(());
        }
        let next = self
            .WithCheckpoints(|vsf| vsf.Min())
            .and_then(|v| v)
            .map(newCheckpointWithSpan)
            .unwrap_or_else(|| newCheckpointWithTS(min_ts));
        self.setCheckpoint(next);
        let checkpoint = self
            .lastCheckpoint
            .lock()
            .unwrap()
            .as_ref()
            .map(|p| p.TS)
            .unwrap_or(min_ts);

        self.env
            .UploadV3GlobalCheckpointForTask(&task.Name, checkpoint)
            .map_err(|e| format!("failed to upload global checkpoint: {e}"))?;

        if self.isCheckpointLagged(&task).unwrap_or(false) {
            self.env
                .PauseTask(&task.Name)
                .map_err(|e| format!("failed to pause task: {e}"))?;
            // Go observes this through the metadata watcher. Set it eagerly as well so
            // another tick cannot advance while that event is in flight.
            self.isPaused.store(true, Ordering::SeqCst);
            return Err("check point lagged too large".into());
        }

        let safe_ts = checkpoint.saturating_sub(1);
        self.env.BlockGCUntil(safe_ts).map_err(|e| {
            format!("failed to update service GC safe point, target checkpoint is {safe_ts}: {e}")
        })?;
        Ok(())
    }

    /// 可选 tick：对任务范围执行 tryAdvance，合并最新 flush TS。
    pub fn optionalTick(&self) -> Result<(), String> {
        // Subscription errors fall back to polling, as in Go optionalTick.
        let _ = self.subscribeTick();
        let ranges = self.taskRange.lock().unwrap().clone();
        if ranges.is_empty() {
            return Ok(());
        }
        let checkpoints = self.checkpoints.clone();
        let hook: OnSuccessHook = Arc::new(move |ts, kr| {
            if let Ok(mut g) = checkpoints.lock() {
                if let Some(cp) = g.as_mut() {
                    cp.Merge(Valued {
                        Key: Span {
                            StartKey: kr.StartKey.clone(),
                            EndKey: kr.EndKey.clone(),
                        },
                        Value: ts,
                    });
                }
            }
        });
        let _ = self.tryAdvance(&ranges, Some(hook))?;
        Ok(())
    }

    /// 单次推进循环：先 optional 再 important；无任务或暂停则跳过。
    pub fn tick(&self) -> Result<(), String> {
        if self.task.lock().unwrap().is_none() || self.isPaused.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut errs = Vec::new();
        // 收集两侧错误后合并返回，与 Go 多错误拼接一致。
        if let Err(e) = self.optionalTick() {
            errs.push(e);
        }
        if let Err(e) = self.importantTick() {
            errs.push(e);
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    /// 关闭外部全局检查点存储；本端口无存储时为空操作。
    pub fn closeGlobalCheckpointStorage(&self) {
        // storage is optional boundary; no-op when absent.
    }

    /// Hold the subscriber lock across the hook and topology/error maintenance.
    /// OnStop must wait for an in-flight subscription tick before dropping it.
    pub(crate) fn subscribeTick(&self) -> Result<(), String> {
        let mut subscriber = self.subscriber.lock().unwrap();
        let Some(subscriber) = subscriber.as_mut() else {
            return Ok(());
        };
        #[cfg(test)]
        {
            let hook = self.subscribeTickHook.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook();
            }
        }
        if let Err(error) = subscriber.UpdateStoreTopology() {
            eprintln!("Error when updating store topology: {error}");
        }
        subscriber.HandleErrors();
        subscriber.PendingErrors()
    }

    #[cfg(test)]
    pub(crate) fn setSubscribeTickHook(&self, hook: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.subscribeTickHook.lock().unwrap() = hook;
    }

    /// 清空并丢弃 flush 订阅器。
    pub fn stopSubscriber(&self) {
        if let Some(mut s) = self.subscriber.lock().unwrap().take() {
            s.Clear();
        }
    }

    /// 安装新的 flush 订阅处理器（成为 Owner 时调用）。
    pub fn SpawnSubscriptionHandler(&self) {
        let sub = crate::flush_subscriber::NewSubscriber(self.env.clone(), Vec::new());
        *self.subscriber.lock().unwrap() = Some(sub);
    }
}

/// 计算 resolve-lock 的目标上界 TSO。
/// 间隔为 0 时退化为检查点 + 固定滞后；否则为 currentTS - 2*interval。
pub fn resolveLockTargetUpperBound(
    checkpointTS: u64,
    resolveLockInterval: Duration,
    currentTS: u64,
) -> u64 {
    if resolveLockInterval.is_zero() {
        return tso_after(checkpointTS, resolveLockRetryLowerBoundLag);
    }
    tso_before_from_ts(currentTS, resolveLockInterval * 2)
}

/// 计算重试时的 maxVersion 下界；仅当下界夹在 (checkpoint, maxVersion) 内才有效。
pub fn resolveLockRetryLowerBound(checkpointTS: u64, maxVersion: u64) -> (u64, bool) {
    let lower = tso_after(checkpointTS, resolveLockRetryLowerBoundLag);
    (lower, lower > checkpointTS && lower < maxVersion)
}

/// 识别 ScanLock 因 key locked 失败的错误串（Go 侧字符串匹配）。
pub fn isScanLockLockedError(err: &str) -> bool {
    err.contains("unexpected scanlock error") && err.contains("locked")
}

/// 将 maxVersion 向 lowerBound 二分下调；间隙不足则放弃重试。
pub fn lowerResolveLockMaxVersion(maxVersion: u64, lowerBound: u64) -> (u64, bool) {
    if maxVersion <= lowerBound || maxVersion - lowerBound <= 1 {
        return (0, false);
    }
    (lowerBound + (maxVersion - lowerBound) / 2, true)
}

/// 带 maxVersion 重试的范围锁解决：locked 错误时下调版本再试，超过次数则失败。
pub fn resolveLocksForRangeWithMaxVersionRetry(
    resolver: &dyn crate::advancer_env::RegionLockResolver,
    maxVersion: u64,
    retryLowerBound: u64,
    retryLowerBoundValid: bool,
    startKey: &[u8],
    endKey: &[u8],
) -> Result<(), String> {
    let mut current = maxVersion;
    for retry in 0.. {
        match resolver.ResolveLocksForRange(current, startKey, endKey) {
            Ok(()) => return Ok(()),
            Err(e) => {
                // 非 locked 或已达重试上限：直接透传错误。
                if !isScanLockLockedError(&e) || retry >= resolveLockMaxVersionMaxRetry {
                    return Err(e);
                }
                if !retryLowerBoundValid {
                    return Err(e);
                }
                match lowerResolveLockMaxVersion(current, retryLowerBound) {
                    (next, true) => current = next,
                    (_, false) => return Err(e),
                }
            }
        }
    }
    Ok(())
}
