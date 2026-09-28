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

// 自动 ANALYZE 优先队列主体。
//
// `AnalysisPriorityQueue` 维护按权重排序的分析作业堆，并启动后台线程周期性：
// - 拉取 DML（数据操纵语言，INSERT/UPDATE/DELETE）变更刷新作业；
// - 将 must-retry 失败作业重新入队；
// - 刷新上次分析时长指标以更新权重。
//
// 数据源由 `QueueSource` 抽象，便于测试注入与对接真实 StatsHandle。

use crate::calculator::{NewPriorityCalculator, PriorityCalculator};
use crate::heap::{NewHeap, PqHeapImpl};
use crate::job::{AnalysisJob, AnalysisJobJSON, FailureJobHook, Indicators, SuccessJobHook};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

/// 队列尚未 Initialize 时的错误信息。
pub const NOT_INITIALIZED_ERR_MSG: &str = "priority queue not initialized";
/// 刷新堆中作业“上次分析时长”的后台间隔（默认 10 分钟）。
pub const LAST_ANALYSIS_DURATION_REFRESH_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// 拉取 DML 导致的统计变更的后台间隔（默认 2 分钟）。
pub const DML_CHANGES_FETCH_INTERVAL: Duration = Duration::from_secs(2 * 60);
/// must-retry 作业重新入队的后台间隔（默认 5 分钟）。
pub const MUST_RETRY_JOB_REQUEUE_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// 慢操作日志阈值（预留，与 Go 对齐）。
pub const SLOW_LOG_THRESHOLD: Duration = Duration::from_millis(150);

/// 队列的外部数据源：构建/重建作业、查询变更与刷新指标。
pub trait QueueSource: Send + Sync + 'static {
    /// 全量扫描表并构建初始分析作业列表。
    fn build_analysis_jobs(&self) -> Result<Vec<Box<dyn AnalysisJob>>, String>;
    /// 根据 DML 变更与当前运行中作业集合，返回需更新的作业。
    fn changed_analysis_jobs(
        &self,
        _running_jobs: &HashSet<i64>,
    ) -> Result<Vec<Box<dyn AnalysisJob>>, String> {
        Ok(Vec::new())
    }
    /// 按表/分区 ID 重新创建单个作业；无则返回 None。
    fn recreate_job(&self, _table_id: i64) -> Result<Option<Box<dyn AnalysisJob>>, String> {
        Ok(None)
    }
    /// 刷新指定作业的 Indicators（尤其是 LastAnalysisDuration）。
    fn refreshed_indicators(&self, _table_id: i64) -> Result<Option<Indicators>, String> {
        Ok(None)
    }
}

/// 队列可变状态：堆、初始化/关闭标志、运行中与 must-retry 集合。
struct QueueState {
    heap: PqHeapImpl,
    initialized: bool,
    /// 已 Pop 但尚未完成的作业表/分区 ID。
    running_jobs: HashSet<i64>,
    /// 失败且需强制重试的作业 ID。
    must_retry_jobs: HashSet<i64>,
}

impl Default for QueueState {
    fn default() -> Self {
        Self {
            heap: NewHeap(),
            initialized: false,
            running_jobs: HashSet::new(),
            must_retry_jobs: HashSet::new(),
        }
    }
}

/// 后台 worker 控制：退出信道与线程句柄。
#[derive(Default)]
struct WorkerControl {
    exit: Option<Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

/// 可克隆的分析优先队列句柄（内部共享状态）。
#[derive(Clone)]
pub struct AnalysisPriorityQueue {
    source: Arc<dyn QueueSource>,
    calculator: Arc<PriorityCalculator>,
    state: Arc<Mutex<QueueState>>,
    worker: Arc<Mutex<WorkerControl>>,
}

/// 创建优先队列；需再调用 Initialize 才会加载作业并启动后台线程。
pub fn NewAnalysisPriorityQueue(source: Arc<dyn QueueSource>) -> AnalysisPriorityQueue {
    AnalysisPriorityQueue {
        source,
        calculator: Arc::new(NewPriorityCalculator()),
        state: Arc::new(Mutex::new(QueueState::default())),
        worker: Arc::new(Mutex::new(WorkerControl::default())),
    }
}

impl AnalysisPriorityQueue {
    /// 获取状态锁；poison 时恢复内层数据以避免永久卡死。
    fn lock_state(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 是否已成功 Initialize。
    pub fn IsInitialized(&self) -> bool {
        self.lock_state().initialized
    }

    /// 首次初始化：Rebuild 全量作业并启动后台 worker；幂等。
    pub fn Initialize(&self) -> Result<(), String> {
        {
            let state = self.lock_state();
            if state.initialized {
                return Ok(());
            }
        }
        let jobs = self.FetchAllTablesAndBuildAnalysisJobs()?;
        self.RebuildWithoutLock(jobs)?;
        self.start_worker();
        Ok(())
    }

    /// 从数据源全量拉取作业并重建堆。
    pub fn Rebuild(&self) -> Result<(), String> {
        {
            let state = self.lock_state();
            Self::require_initialized(&state)?;
        }
        let jobs = self.FetchAllTablesAndBuildAnalysisJobs()?;
        self.RebuildWithoutLock(jobs)
    }

    /// 委托 QueueSource 构建全部分析作业。
    pub fn FetchAllTablesAndBuildAnalysisJobs(&self) -> Result<Vec<Box<dyn AnalysisJob>>, String> {
        self.source.build_analysis_jobs()
    }

    /// 用给定作业列表重置堆与运行/重试集合，并标记已初始化。
    pub fn RebuildWithoutLock(&self, jobs: Vec<Box<dyn AnalysisJob>>) -> Result<(), String> {
        let mut state = self.lock_state();
        state.heap = NewHeap();
        state.running_jobs.clear();
        state.must_retry_jobs.clear();
        for job in jobs {
            Self::push_locked(&self.state, &self.calculator, &mut state, job)?;
        }
        state.initialized = true;
        Ok(())
    }

    /// 仅启动后台 worker（不重建作业）。
    pub fn Run(&self) {
        self.start_worker();
    }

    /// 启动 1 秒 tick 后台循环，按间隔调用三类刷新逻辑。
    fn start_worker(&self) {
        let mut control = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if control.worker.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        control.exit = Some(sender);
        let queue = self.clone();
        control.worker = Some(std::thread::spawn(move || {
            let mut dml_elapsed = Duration::ZERO;
            let mut retry_elapsed = Duration::ZERO;
            let mut refresh_elapsed = Duration::ZERO;
            let tick = Duration::from_secs(1);
            loop {
                // 收到退出信号或信道断开则结束循环。
                match receiver.recv_timeout(tick) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                dml_elapsed += tick;
                retry_elapsed += tick;
                refresh_elapsed += tick;
                if dml_elapsed >= DML_CHANGES_FETCH_INTERVAL {
                    let _ = queue.ProcessDMLChanges();
                    dml_elapsed = Duration::ZERO;
                }
                if retry_elapsed >= MUST_RETRY_JOB_REQUEUE_INTERVAL {
                    let _ = queue.RequeueMustRetryJobs();
                    retry_elapsed = Duration::ZERO;
                }
                if refresh_elapsed >= LAST_ANALYSIS_DURATION_REFRESH_INTERVAL {
                    let _ = queue.RefreshLastAnalysisDuration();
                    refresh_elapsed = Duration::ZERO;
                }
            }
        }));
    }

    fn require_initialized(state: &QueueState) -> Result<(), String> {
        if state.initialized {
            Ok(())
        } else {
            Err(NOT_INITIALIZED_ERR_MSG.to_owned())
        }
    }

    /// 注册成功/失败钩子：维护 running_jobs 与 must_retry_jobs。
    fn hooks(state: Weak<Mutex<QueueState>>) -> (SuccessJobHook, FailureJobHook) {
        let success_state = state.clone();
        let success: SuccessJobHook = Arc::new(move |job| {
            let table_id = job.GetTableID();
            if let Some(state) = success_state.upgrade() {
                let mut state = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.running_jobs.remove(&table_id);
            }
        });
        let failure: FailureJobHook = Arc::new(move |job, must_retry| {
            let table_id = job.GetTableID();
            if let Some(state) = state.upgrade() {
                let mut state = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.running_jobs.remove(&table_id);
                if must_retry {
                    state.must_retry_jobs.insert(table_id);
                }
            }
        });
        (success, failure)
    }

    /// 计算权重、挂钩子后写入堆（调用方已持有 state 可变借用）。
    fn push_locked(
        shared_state: &Arc<Mutex<QueueState>>,
        calculator: &PriorityCalculator,
        state: &mut QueueState,
        mut job: Box<dyn AnalysisJob>,
    ) -> Result<(), String> {
        let weight = calculator.CalculateWeight(job.as_ref());
        job.SetWeight(weight);
        let (success, failure) = Self::hooks(Arc::downgrade(shared_state));
        job.RegisterSuccessHook(success);
        job.RegisterFailureHook(failure);
        state.heap.AddOrUpdate(job)
    }

    /// 推入作业；若该 ID 已在 running_jobs 中则跳过（避免重复调度）。
    pub fn Push(&self, job: Box<dyn AnalysisJob>) -> Result<(), String> {
        let mut state = self.lock_state();
        Self::require_initialized(&state)?;
        if state.running_jobs.contains(&job.GetTableID()) {
            return Ok(());
        }
        Self::push_locked(&self.state, &self.calculator, &mut state, job)
    }

    /// 与 Push 相同（保留 Go 命名兼容）。
    pub fn PushWithoutLock(&self, job: Box<dyn AnalysisJob>) -> Result<(), String> {
        self.Push(job)
    }

    /// 若 job 为 Some 则 Push，否则无操作。
    pub fn TryCreateJob(&self, job: Option<Box<dyn AnalysisJob>>) -> Result<(), String> {
        if let Some(job) = job {
            self.Push(job)?;
        }
        Ok(())
    }

    /// 更新已有作业（实质为 Push/AddOrUpdate）。
    pub fn TryUpdateJob(&self, job: Box<dyn AnalysisJob>) -> Result<(), String> {
        self.Push(job)
    }

    /// 批量用表统计变更更新队列。
    pub fn ProcessTableStats(&self, jobs: Vec<Box<dyn AnalysisJob>>) -> Result<(), String> {
        for job in jobs {
            self.TryUpdateJob(job)?;
        }
        Ok(())
    }

    /// 处理后台 DML 变更：跳过仍在 running 的作业。
    pub fn ProcessDMLChanges(&self) -> Result<(), String> {
        let running = self.GetRunningJobs();
        let jobs = self.source.changed_analysis_jobs(&running)?;
        let mut state = self.lock_state();
        Self::require_initialized(&state)?;
        for job in jobs {
            if !state.running_jobs.contains(&job.GetTableID()) {
                Self::push_locked(&self.state, &self.calculator, &mut state, job)?;
            }
        }
        Ok(())
    }

    /// 将 must_retry_jobs 中仍可重建的作业重新 Push 并移出集合。
    pub fn RequeueMustRetryJobs(&self) -> Result<(), String> {
        let ids = {
            let state = self.lock_state();
            Self::require_initialized(&state)?;
            state.must_retry_jobs.iter().copied().collect::<Vec<_>>()
        };
        for table_id in ids {
            // Go consumes the retry marker before attempting recreation. A missing table or
            // recreation failure is not retained forever; future DML/DDL changes may enqueue it.
            self.lock_state().must_retry_jobs.remove(&table_id);
            if let Some(job) = self.source.recreate_job(table_id)? {
                let mut state = self.lock_state();
                if !state.running_jobs.contains(&table_id) {
                    Self::push_locked(&self.state, &self.calculator, &mut state, job)?;
                }
            }
        }
        Ok(())
    }

    /// 刷新堆中各作业的上次分析时长，并按更新后的指标重新计算权重。
    pub fn RefreshLastAnalysisDuration(&self) -> Result<(), String> {
        let ids = {
            let state = self.lock_state();
            Self::require_initialized(&state)?;
            state.heap.ListKeys()
        };
        for table_id in ids {
            let Some(indicators) = self.source.refreshed_indicators(table_id)? else {
                continue;
            };
            let mut state = self.lock_state();
            let Ok(mut job) = state.heap.DeleteByKey(table_id) else {
                continue;
            };
            // Go refreshes only this field on the existing job. Preserve the remaining
            // indicators and registered completion hooks, then restore heap order.
            let mut updated = job.GetIndicators();
            updated.LastAnalysisDuration = indicators.LastAnalysisDuration;
            job.SetIndicators(updated);
            job.SetWeight(self.calculator.CalculateWeight(job.as_ref()));
            state.heap.Update(job)?;
        }
        Ok(())
    }

    /// 返回当前 running_jobs 快照。
    pub fn GetRunningJobs(&self) -> HashSet<i64> {
        self.lock_state().running_jobs.clone()
    }

    /// 弹出权重最高的作业，并记入 running_jobs。
    pub fn Pop(&self) -> Result<Box<dyn AnalysisJob>, String> {
        let mut state = self.lock_state();
        Self::require_initialized(&state)?;
        let job = state.heap.Pop()?;
        state.running_jobs.insert(job.GetTableID());
        Ok(job)
    }

    /// 测试用：窥视堆顶作业的 JSON 视图。
    pub fn PeekForTest(&self) -> Result<AnalysisJobJSON, String> {
        let state = self.lock_state();
        Self::require_initialized(&state)?;
        Ok(state.heap.Peek()?.AsJSON())
    }

    /// 测试用：堆是否为空。
    pub fn IsEmptyForTest(&self) -> Result<bool, String> {
        let state = self.lock_state();
        Self::require_initialized(&state)?;
        Ok(state.heap.IsEmpty())
    }

    /// 堆中作业数量。
    pub fn Len(&self) -> Result<usize, String> {
        let state = self.lock_state();
        Self::require_initialized(&state)?;
        Ok(state.heap.Len())
    }

    /// 快照：堆中作业 JSON 列表 + running + must_retry 集合。
    pub fn Snapshot(&self) -> Result<(Vec<AnalysisJobJSON>, HashSet<i64>, HashSet<i64>), String> {
        let state = self.lock_state();
        Self::require_initialized(&state)?;
        Ok((
            state
                .heap
                .List()
                .into_iter()
                .map(AnalysisJob::AsJSON)
                .collect(),
            state.running_jobs.clone(),
            state.must_retry_jobs.clone(),
        ))
    }

    /// 按表/分区 ID 删除堆中作业，并清理 must_retry 条目。
    pub fn DeleteByTableID(&self, table_id: i64) -> Result<(), String> {
        let mut state = self.lock_state();
        Self::require_initialized(&state)?;
        if state.heap.GetByKey(table_id).is_some() {
            state.heap.DeleteByKey(table_id)?;
        }
        state.must_retry_jobs.remove(&table_id);
        Ok(())
    }

    /// 删除后按数据源重建并 Push。
    pub fn RecreateAndPushJob(&self, table_id: i64) -> Result<(), String> {
        self.DeleteByTableID(table_id)?;
        if let Some(job) = self.source.recreate_job(table_id)? {
            self.Push(job)?;
        }
        Ok(())
    }

    /// 停止后台 worker、清空堆与集合，并标记 closed。
    pub fn Close(&self) {
        let worker = {
            let mut control = self
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // drop exit sender 触发 worker 退出。
            control.exit.take();
            control.worker.take()
        };
        if let Some(worker) = worker {
            let _ = worker.join();
        }
        let mut state = self.lock_state();
        state.initialized = false;
        state.heap = NewHeap();
        state.running_jobs.clear();
        state.must_retry_jobs.clear();
    }

    /// 测试/重置用：清空同步字段但不关闭 worker。
    pub fn ResetSyncFields(&self) {
        let mut state = self.lock_state();
        state.initialized = false;
        state.heap = NewHeap();
        state.running_jobs.clear();
        state.must_retry_jobs.clear();
    }
}

impl Drop for AnalysisPriorityQueue {
    /// 最后一个强引用释放时自动 Close，避免后台线程泄漏。
    fn drop(&mut self) {
        if Arc::strong_count(&self.worker) == 1 {
            self.Close();
        }
    }
}

/// 运行中作业集合的别名类型（与 Go map[int64]struct{} 对齐）。
pub type RunningJobs = HashMap<i64, ()>;
