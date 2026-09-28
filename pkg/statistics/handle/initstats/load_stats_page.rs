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

// 按表 ID 区间并发加载统计的 RangeWorker。
//
// 将表 ID 范围拆成 `Task` 投递到有界通道，多个 worker 线程消费并调用
// `processTask`；同时更新全局 `InitStatsPercentage` 与采样进度日志。
// 非 lite 模式下用于展示初始化加载进度百分比。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender, bounded};

use crate::util::logutil::{BgLogger, LogField, LogFieldCategory, Logger, sample_logger_factory};

/// 用 `AtomicU64` 存 `f64` 比特位的原子浮点，便于跨线程更新进度百分比。
pub struct AtomicFloat64 {
    bits: AtomicU64,
}

impl AtomicFloat64 {
    /// 以给定浮点初值构造。
    pub const fn new(value: f64) -> Self {
        Self {
            bits: AtomicU64::new(value.to_bits()),
        }
    }

    /// 原子读取当前浮点值。
    pub fn Load(&self) -> f64 {
        f64::from_bits(self.bits.load(Ordering::SeqCst))
    }

    /// 原子写入浮点值。
    pub fn Store(&self, value: f64) {
        self.bits.store(value.to_bits(), Ordering::SeqCst);
    }
}

// InitStatsPercentage is the percentage of the table to load stats.
// This only works for non-lite mode.
/// 初始化加载统计的全局进度百分比（非 lite 模式有效）。
pub static InitStatsPercentage: AtomicFloat64 = AtomicFloat64::new(0.0);

/// 进程内单例：带 60s 采样与 `stats` 分类字段的后台进度日志器。
fn singletonStatsSamplerLogger() -> Logger {
    static LOGGER: OnceLock<Logger> = OnceLock::new();
    LOGGER
        .get_or_init(|| {
            sample_logger_factory(
                BgLogger(),
                Duration::from_secs(60),
                1,
                vec![LogField::String(
                    LogFieldCategory.to_owned(),
                    "stats".to_owned(),
                )],
            )()
        })
        .clone()
}

// Task represents the range of the table for loading stats.
/// 一次加载任务覆盖的表 ID 半开/闭区间端点（`StartTid`..`EndTid`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Task {
    pub StartTid: i64,
    pub EndTid: i64,
}

/// 处理单个 `Task` 的回调类型（可跨线程共享）。
type ProcessTask = dyn Fn(Task) -> Result<()> + Send + Sync + 'static;

// RangeWorker is used to load stats concurrently by the range of table id.
/// 按表 ID 区间并发加载统计的 worker 池与任务通道。
pub struct RangeWorker {
    progressLogger: Option<Logger>,

    taskName: Arc<str>,
    taskSender: Mutex<Option<Sender<Task>>>,
    taskReceiver: Receiver<Task>,
    processTask: Arc<ProcessTask>,
    taskCnt: u64,
    completeTaskCnt: Arc<AtomicU64>,

    totalPercentage: f64,
    totalPercentageStep: f64,

    concurrency: usize,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

// NewRangeWorker creates a new RangeWorker.
/// 创建 RangeWorker：有界任务通道容量为 1，并记录本阶段进度步长。
pub fn NewRangeWorker<F>(
    task_name: String,
    process_task: F,
    concurrency: usize,
    total_task_cnt: u64,
    total_percentage_step: f64,
) -> RangeWorker
where
    F: Fn(Task) -> Result<()> + Send + Sync + 'static,
{
    let (task_sender, task_receiver) = bounded(1);
    RangeWorker {
        progressLogger: Some(singletonStatsSamplerLogger()),
        taskName: Arc::from(task_name),
        taskSender: Mutex::new(Some(task_sender)),
        taskReceiver: task_receiver,
        processTask: Arc::new(process_task),
        taskCnt: total_task_cnt,
        completeTaskCnt: Arc::new(AtomicU64::new(0)),
        // 继承当前全局进度，再在本阶段按 step 累加。
        totalPercentage: InitStatsPercentage.Load(),
        totalPercentageStep: total_percentage_step,
        concurrency,
        workers: Mutex::new(Vec::with_capacity(concurrency)),
    }
}

impl RangeWorker {
    // LoadStats loads stats concurrently when to init stats.
    /// 启动 `concurrency` 个线程，从通道消费任务并加载统计。
    pub fn LoadStats(&self) {
        let mut workers = self.workers.lock().expect("worker list mutex poisoned");
        for _ in 0..self.concurrency {
            let task_receiver = self.taskReceiver.clone();
            let process_task = Arc::clone(&self.processTask);
            let complete_task_cnt = Arc::clone(&self.completeTaskCnt);
            let progress_logger = self.progressLogger.clone();
            let task_name = Arc::clone(&self.taskName);
            let task_cnt = self.taskCnt;
            let total_percentage = self.totalPercentage;
            let total_percentage_step = self.totalPercentageStep;
            workers.push(thread::spawn(move || {
                Self::loadStats(
                    task_receiver,
                    process_task,
                    complete_task_cnt,
                    progress_logger,
                    task_name,
                    task_cnt,
                    total_percentage,
                    total_percentage_step,
                );
            }));
        }
    }

    /// 单 worker 循环：处理任务、累加完成数并刷新全局进度百分比。
    #[allow(clippy::too_many_arguments)]
    fn loadStats(
        task_receiver: Receiver<Task>,
        process_task: Arc<ProcessTask>,
        complete_task_cnt: Arc<AtomicU64>,
        progress_logger: Option<Logger>,
        task_name: Arc<str>,
        task_cnt: u64,
        total_percentage: f64,
        total_percentage_step: f64,
    ) {
        for task in task_receiver {
            if let Err(err) = process_task(task) {
                BgLogger().error(format!("load stats failed: {err:#}"));
            }
            if let Some(progress_logger) = &progress_logger {
                let completed = complete_task_cnt.fetch_add(1, Ordering::SeqCst) + 1;
                // 本阶段完成比例 * 步长 + 进入本阶段前的累计百分比。
                let task_percentage =
                    completed as f64 / task_cnt as f64 * total_percentage_step + total_percentage;
                InitStatsPercentage.Store(task_percentage);
                progress_logger.info(format!("load {task_name} [{completed}/{task_cnt}]"));
            }
        }
    }

    // SendTask sends a task to the load stats worker.
    /// 向任务通道发送一个表 ID 区间任务（通道未关闭时阻塞至可发送）。
    pub fn SendTask(&self, task: Task) {
        let sender = self
            .taskSender
            .lock()
            .expect("task sender mutex poisoned")
            .as_ref()
            .cloned()
            .expect("task channel is closed");
        sender.send(task).expect("task channel is closed");
    }

    // Wait closes the load stats worker.
    /// 关闭发送端并 join 全部 worker，等待通道排空后退出。
    pub fn Wait(&self) {
        // take sender 后 drop，使 receiver 侧 for 循环结束。
        let sender = self
            .taskSender
            .lock()
            .expect("task sender mutex poisoned")
            .take()
            .expect("task channel is already closed");
        drop(sender);

        let workers =
            std::mem::take(&mut *self.workers.lock().expect("worker list mutex poisoned"));
        for worker in workers {
            worker.join().expect("load stats worker panicked");
        }
    }

    /// 已完成任务计数（原子读取）。
    pub fn completed_task_count(&self) -> u64 {
        self.completeTaskCnt.load(Ordering::SeqCst)
    }
}
