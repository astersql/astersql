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

// 自动 ANALYZE 优先级刷新器（Refresher）。
//
// 从 `JobSource` 拉取初始化与 DML 变更产生的分析作业，按优先级入堆，
// 在配置的时间窗口内提交给 Worker；槽位不足时暂存到 must_retry 队列。

use crate::{AnalysisJob, Worker};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

/// 堆中包装的作业：按 priority 最大堆排序，priority 相同时 table_id 较小者优先。
#[derive(Clone, Debug, Eq, PartialEq)]
struct QueuedJob(AnalysisJob);
impl Ord for QueuedJob {
    fn cmp(&self, other: &Self) -> Ordering {
        // 先比优先级（大者优先），再反比 table_id 以保持稳定次序。
        self.0
            .priority
            .cmp(&other.0.priority)
            .then_with(|| other.0.table_id.cmp(&self.0.table_id))
    }
}
impl PartialOrd for QueuedJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 作业来源：初始化扫描、DML 变更增量，以及期望并发度。
pub trait JobSource: Send + Sync {
    /// 首次初始化时扫描需要 ANALYZE 的表，返回作业列表。
    fn initialize(&self) -> Result<Vec<AnalysisJob>, String>;
    /// 根据统计 delta（DML 变化量）生成新的分析作业。
    fn process_dml_changes(&self) -> Result<Vec<AnalysisJob>, String>;
    /// 当前期望的 worker 并发度。
    fn desired_concurrency(&self) -> usize;
}
/// 自动 ANALYZE 刷新器：维护优先队列、时间窗口与 worker 生命周期。
pub struct Refresher {
    /// 并发执行作业的 worker。
    worker: Worker,
    /// 作业来源（初始化 / DML / 并发配置）。
    source: Arc<dyn JobSource>,
    /// 待调度作业的最大堆。
    queue: Mutex<BinaryHeap<QueuedJob>>,
    /// 因槽位不足暂存、需再次入队的作业。
    must_retry: Mutex<Vec<AnalysisJob>>,
    /// 队列是否已完成首次 initialize。
    initialized: AtomicBool,
    /// 串行化首次初始化，避免并发调用重复拉取初始化作业。
    initialize_lock: Mutex<()>,
    /// 是否已关闭（关闭后不再调度）。
    closed: AtomicBool,
    /// 允许执行自动 ANALYZE 的日内时间窗口（起止分钟，可跨午夜）。
    time_window: Mutex<(u32, u32)>,
}
impl Refresher {
    /// 构造刷新器；默认时间窗口为全天 [0, 24*60)。
    pub fn new(worker: Worker, source: Arc<dyn JobSource>) -> Self {
        Self {
            worker,
            source,
            queue: Mutex::new(BinaryHeap::new()),
            must_retry: Mutex::new(Vec::new()),
            initialized: AtomicBool::new(false),
            initialize_lock: Mutex::new(()),
            closed: AtomicBool::new(false),
            time_window: Mutex::new((0, 24 * 60)),
        }
    }
    /// 按 JobSource 期望值同步 worker 并发度。
    pub fn update_concurrency(&self) {
        self.worker
            .update_concurrency(self.source.desired_concurrency());
    }
    /// 设置自动分析时间窗口（单位：一天内的分钟数）。
    pub fn set_auto_analysis_time_window(
        &self,
        start_minute: u32,
        end_minute: u32,
    ) -> Result<(), String> {
        if start_minute >= 24 * 60 || end_minute > 24 * 60 {
            return Err("invalid auto analyze time window".into());
        }
        *self.time_window.lock().expect("time window mutex poisoned") = (start_minute, end_minute);
        Ok(())
    }
    /// 判断给定分钟是否落在时间窗口内；start > end 表示跨午夜窗口。
    pub fn is_within_time_window(&self, minute: u32) -> bool {
        let (start, end) = *self.time_window.lock().expect("time window mutex poisoned");
        if start <= end {
            minute >= start && minute < end
        } else {
            // 跨午夜：例如 23:00–02:00。
            minute >= start || minute < end
        }
    }
    /// 在时间窗口内取出最高优先级作业并提交；返回是否成功提交。
    pub fn analyze_highest_priority_tables(&self, minute: u32) -> Result<bool, String> {
        if self.closed.load(AtomicOrdering::Acquire) {
            return Ok(false);
        }
        self.initialize_queue()?;
        // 与 Go 实现一样，初始化必须发生在时间窗检查之前；否则实例在
        // 时间窗外启动时无法接收 DDL 事件，队列也不会前进。
        if !self.is_within_time_window(minute) {
            return Ok(false);
        }
        self.process_dml_changes()?;
        self.requeue_must_retry();
        self.update_concurrency();

        let running = self.worker.running_jobs();
        let remaining = self.worker.max_concurrency().saturating_sub(running.len());
        let mut submitted = 0;
        while submitted < remaining {
            let Some(job) = self
                .queue
                .lock()
                .expect("queue mutex poisoned")
                .pop()
                .map(|value| value.0)
            else {
                break;
            };
            if running.contains(&job.table_id) {
                continue;
            }
            // 提交失败（并发度在检查后被其他调用占用）则放入 must_retry，
            // 下一轮再入堆；这对应 Go queue 的 must-retry 路径。
            if self.worker.submit_job(job.clone()) {
                submitted += 1;
            } else {
                self.must_retry
                    .lock()
                    .expect("retry mutex poisoned")
                    .push(job);
                break;
            }
        }
        Ok(submitted > 0)
    }

    fn initialize_queue(&self) -> Result<(), String> {
        if self.initialized.load(AtomicOrdering::Acquire) {
            return Ok(());
        }
        let _guard = self
            .initialize_lock
            .lock()
            .expect("initialize mutex poisoned");
        if self.initialized.load(AtomicOrdering::Acquire) {
            return Ok(());
        }
        let jobs = self.source.initialize()?;
        let mut queue = self.queue.lock().expect("queue mutex poisoned");
        for job in jobs {
            queue.push(QueuedJob(job));
        }
        self.initialized.store(true, AtomicOrdering::Release);
        Ok(())
    }
    /// 队列已初始化时，拉取 DML 变更产生的作业并入优先队列。
    ///
    /// 对应 Go 的 `ProcessDMLChangesForTest`：初始化前调用必须为空操作，
    /// 以免提前消费尚不能由优先队列处理的增量。
    pub fn process_dml_changes(&self) -> Result<(), String> {
        if !self.initialized.load(AtomicOrdering::Acquire) {
            return Ok(());
        }
        for job in self.source.process_dml_changes()? {
            self.queue
                .lock()
                .expect("queue mutex poisoned")
                .push(QueuedJob(job));
        }
        Ok(())
    }
    /// 将 must_retry 中的作业重新压回优先队列。
    pub fn requeue_must_retry(&self) {
        let jobs = std::mem::take(&mut *self.must_retry.lock().expect("retry mutex poisoned"));
        let mut queue = self.queue.lock().expect("queue mutex poisoned");
        for job in jobs {
            queue.push(QueuedJob(job));
        }
    }
    /// 返回当前优先队列中作业的无序快照（用于测试/诊断）。
    pub fn priority_queue_snapshot(&self) -> Vec<AnalysisJob> {
        self.queue
            .lock()
            .expect("queue mutex poisoned")
            .iter()
            .map(|job| job.0.clone())
            .collect()
    }
    /// 转发 worker 当前运行中的表 ID 集合。
    pub fn running_jobs(&self) -> HashSet<i64> {
        self.worker.running_jobs()
    }
    /// 等待 worker 全部作业结束。
    pub fn wait_finished(&self) {
        self.worker.wait_finished();
    }
    /// 队列是否已完成首次 initialize。
    pub fn is_queue_initialized(&self) -> bool {
        self.initialized.load(AtomicOrdering::Acquire)
    }
    /// 优先队列当前长度。
    pub fn len(&self) -> usize {
        self.queue.lock().expect("queue mutex poisoned").len()
    }
    /// 清空优先队列。
    pub fn close_priority_queue(&self) {
        let _guard = self
            .initialize_lock
            .lock()
            .expect("initialize mutex poisoned");
        self.queue.lock().expect("queue mutex poisoned").clear();
        self.must_retry
            .lock()
            .expect("retry mutex poisoned")
            .clear();
        self.initialized.store(false, AtomicOrdering::Release);
    }
    /// 关闭刷新器：清空队列并停止 worker（幂等）。
    pub fn close(&self) {
        if !self.closed.swap(true, AtomicOrdering::AcqRel) {
            self.close_priority_queue();
            self.worker.stop();
        }
    }
}
