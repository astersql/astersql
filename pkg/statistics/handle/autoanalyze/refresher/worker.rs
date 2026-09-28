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

// 自动 ANALYZE 作业并发执行 worker。
//
// 以受限并发度提交 `AnalysisJob`，在独立线程中调用 `AnalysisExecutor`，
// 并跟踪运行中表 ID，供 refresher 查询与优雅停止。

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// 待执行的自动 ANALYZE 作业（按表 ID 与优先级描述）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalysisJob {
    /// 物理表 ID（分区表场景下可为分区 ID）。
    pub table_id: i64,
    /// 调度优先级；数值越大越优先。
    pub priority: i64,
    /// 是否必须在失败后重试入队。
    pub must_retry: bool,
}
/// 实际执行 ANALYZE 的执行器抽象（对应 Go 侧对 StatsHandle 的调用）。
pub trait AnalysisExecutor: Send + Sync {
    /// 执行单个分析作业。
    fn analyze(&self, job: &AnalysisJob) -> Result<(), String>;
}

/// 受限并发的 ANALYZE worker：控制活跃槽位、运行集合与停止信号。
pub struct Worker {
    /// 作业执行器。
    executor: Arc<dyn AnalysisExecutor>,
    /// 最大并发度；与 Go worker 一样，0 表示不接受任何作业。
    max_concurrency: Arc<AtomicUsize>,
    /// 当前活跃作业数。
    active: Arc<AtomicUsize>,
    /// 正在运行的表 ID 集合。
    running: Arc<Mutex<HashSet<i64>>>,
    /// 作业完成时唤醒 `wait_finished` 等待方。
    finished: Arc<Condvar>,
    /// 已停止后拒绝新提交。
    stopped: Arc<AtomicBool>,
}
impl Worker {
    /// 构造 worker。
    pub fn new(executor: Arc<dyn AnalysisExecutor>, max_concurrency: usize) -> Self {
        Self {
            executor,
            max_concurrency: Arc::new(AtomicUsize::new(max_concurrency)),
            active: Arc::new(AtomicUsize::new(0)),
            running: Arc::new(Mutex::new(HashSet::new())),
            finished: Arc::new(Condvar::new()),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }
    /// 运行时更新并发上限。
    pub fn update_concurrency(&self, value: usize) {
        self.max_concurrency.store(value, Ordering::Release);
    }
    /// 尝试提交作业：槽位满或已停止时返回 false；成功则异步执行。
    pub fn submit_job(&self, job: AnalysisJob) -> bool {
        if self.stopped.load(Ordering::Acquire) {
            return false;
        }
        // CAS 占用一个并发槽；若已达上限则拒绝提交。
        let mut current = self.active.load(Ordering::Acquire);
        loop {
            if current >= self.max_concurrency.load(Ordering::Acquire) {
                return false;
            }
            match self.active.compare_exchange(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        self.running
            .lock()
            .expect("running jobs mutex poisoned")
            .insert(job.table_id);
        let executor = Arc::clone(&self.executor);
        let active = Arc::clone(&self.active);
        let running = Arc::clone(&self.running);
        let finished = Arc::clone(&self.finished);
        let table_id = job.table_id;
        // 在独立线程中执行 ANALYZE。Go 的 WaitGroupWrapper 会恢复执行器
        // panic；无论返回错误还是 panic，都必须清理 running 状态。
        let spawned = std::thread::Builder::new().spawn(move || {
            let _ = catch_unwind(AssertUnwindSafe(|| executor.analyze(&job)));
            running
                .lock()
                .expect("running jobs mutex poisoned")
                .remove(&table_id);
            active.fetch_sub(1, Ordering::AcqRel);
            finished.notify_all();
        });
        if spawned.is_err() {
            self.running
                .lock()
                .expect("running jobs mutex poisoned")
                .remove(&table_id);
            self.active.fetch_sub(1, Ordering::AcqRel);
            self.finished.notify_all();
            return false;
        }
        true
    }
    /// 返回当前运行中的表 ID 快照。
    pub fn running_jobs(&self) -> HashSet<i64> {
        self.running
            .lock()
            .expect("running jobs mutex poisoned")
            .clone()
    }
    /// 当前最大并发度。
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency.load(Ordering::Acquire)
    }
    /// 标记停止并等待所有运行中作业结束。
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.wait_finished();
    }
    /// 阻塞直到运行集合为空。
    pub fn wait_finished(&self) {
        let mut running = self.running.lock().expect("running jobs mutex poisoned");
        while !running.is_empty() {
            running = self
                .finished
                .wait(running)
                .expect("running jobs mutex poisoned");
        }
    }
}
