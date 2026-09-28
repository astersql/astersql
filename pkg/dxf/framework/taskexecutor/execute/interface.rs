// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 任务执行器（Task Executor）单步执行接口与子任务运行摘要。
//
// `StepExecutor` 定义一步内各 subtask 的 Init/Run/Cleanup 生命周期；
// `SubtaskSummary` 采样进度并估算速度；框架通过 `FrameworkInfo` 注入
// step、资源（CPU/内存配额）、计量与 checkpoint 回调。

#![allow(non_snake_case, non_upper_case_globals)]

use anyhow::Result;
use proto::step::Step;
use proto::subtask::{StepResource, Subtask};
use proto::task::Task;
use std::any::Any;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};
use tokio_util::sync::CancellationToken;

/// 跨执行回调传递取消信号（对应 Go 的 context.Context）。
/// Context carries cancellation across executor callbacks, as Go context.Context does.
pub type Context = CancellationToken;

/// 单任务步骤内所有子任务的执行器接口。
///
/// 框架依次调用 Init、对每个 subtask 调用 RunSubtask，最后 Cleanup。
/// StepExecutor defines the executor for subtasks of one task step.
///
/// The framework calls Init, then RunSubtask for each subtask, and finally Cleanup.
pub trait StepExecutor: StepExecFrameworkInfo {
    fn Init(&mut self, ctx: Context) -> Result<()>;
    fn RunSubtask(&mut self, ctx: Context, subtask: &mut Subtask) -> Result<()>;
    fn RealtimeSummary(&mut self) -> Option<&SubtaskSummary>;
    fn ResetSummary(&mut self);
    fn Cleanup(&mut self, ctx: Context) -> Result<()>;
    fn TaskMetaModified(&mut self, ctx: Context, new_meta: Vec<u8>) -> Result<()>;
    fn ResourceModified(&mut self, ctx: Context, new_resource: &StepResource) -> Result<()>;

    /// 安装框架侧信息（Go 通过 embedding 注入）。
    /// Installs the framework-owned information that Go injects through embedding.
    fn SetFrameworkInfo(&mut self, info: FrameworkInfo);
}

/// 持久化最新子任务摘要的间隔。
/// Interval for persisting the latest subtask summary.
pub const UpdateSubtaskSummaryInterval: Duration = Duration::from_secs(3);
/// 摘要中保留的最大进度采样点数。
const maxProgressInSummary: usize = 5;
/// 平滑速度更新间隔（= 摘要间隔 × 采样点数）。
/// Interval for updating the smoothed subtask speed.
pub const SubtaskSpeedUpdateInterval: Duration =
    Duration::from_secs(UpdateSubtaskSummaryInterval.as_secs() * maxProgressInSummary as u64);

/// 一次子任务进度采样点。
/// Progress is one sampled subtask progress point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    /// 预留字段，与 Go 对齐。
    /// Retained for future use, matching the Go field.
    pub RowCnt: i64,
    /// 已处理的通用计量单位；持久化 JSON 仍用历史字段名 `bytes`。
    /// Generic processed units. Persisted JSON keeps the historical `bytes` name.
    pub Processed: i64,
    /// 采样时间。
    pub UpdateTime: SystemTime,
}

/// 子任务运行时摘要：行数、处理量、对象存储请求与进度采样。
/// SubtaskSummary tracks the runtime summary of a subtask.
#[derive(Debug, Default)]
pub struct SubtaskSummary {
    pub RowCnt: AtomicI64,
    pub Processed: AtomicI64,
    pub ReadBytes: AtomicI64,
    pub GetReqCnt: AtomicU64,
    pub PutReqCnt: AtomicU64,
    pub Progresses: Vec<Progress>,
}

impl SubtaskSummary {
    /// 合并对象存储（S3 等）请求计数快照到摘要。
    /// MergeObjStoreRequests merges a snapshot of object-store requests.
    pub fn MergeObjStoreRequests(&self, reqs: &recording::Requests) {
        let (get, put) = reqs.snapshot();
        self.GetReqCnt.fetch_add(get, Ordering::SeqCst);
        self.PutReqCnt.fetch_add(put, Ordering::SeqCst);
    }

    /// 采样当前计数，并只保留最近五次进度点。
    /// Update samples current counters and retains the latest five samples.
    pub fn Update(&mut self) {
        self.Progresses.push(Progress {
            RowCnt: self.RowCnt.load(Ordering::SeqCst),
            Processed: self.Processed.load(Ordering::SeqCst),
            UpdateTime: SystemTime::now(),
        });

        if self.Progresses.len() > maxProgressInSummary {
            let remove_count = self.Progresses.len() - maxProgressInSummary;
            self.Progresses.drain(..remove_count);
        }
    }

    /// 按采样段与查询时间窗的重叠比例估算处理速度。
    /// GetSpeedInTimeRange estimates speed from the overlap of sampled segments.
    pub fn GetSpeedInTimeRange(&self, end_time: SystemTime, duration: Duration) -> i64 {
        if self.Progresses.len() < 2 || duration.is_zero() {
            return 0;
        }

        let Some(start_time) = end_time.checked_sub(duration) else {
            return 0;
        };
        let first_time = self.Progresses[0].UpdateTime;
        let last_time = self.Progresses[self.Progresses.len() - 1].UpdateTime;
        if end_time < first_time || start_time > last_time {
            return 0;
        }

        // 对相邻采样段按与 [start,end] 的时间重叠比例累加 processed。
        let mut total_processed = 0.0_f64;
        for points in self.Progresses.windows(2) {
            let range_start = points[0].UpdateTime;
            let range_end = points[1].UpdateTime;
            // Go int64 subtraction wraps when the atomic processed counter
            // crosses its boundary; keep the same behavior in debug builds.
            let range_processed = points[1].Processed.wrapping_sub(points[0].Processed) as f64;
            if end_time < range_start || start_time > range_end {
                continue;
            } else if start_time < range_start && end_time > range_end {
                total_processed += range_processed;
                continue;
            }

            let interval_start = start_time.max(range_start);
            let interval_end = end_time.min(range_end);
            let overlap = interval_end
                .duration_since(interval_start)
                .unwrap_or_default()
                .as_secs_f64();
            let full_range = range_end
                .duration_since(range_start)
                .unwrap_or_default()
                .as_secs_f64();
            if full_range > 0.0 {
                total_processed += range_processed * overlap / full_range;
            }
        }

        (total_processed / duration.as_secs_f64()) as i64
    }

    /// 返回最近一次采样时间；无采样时用 UNIX_EPOCH。
    /// UpdateTime returns the last sample time, or the Rust zero-time sentinel.
    pub fn UpdateTime(&self) -> SystemTime {
        self.Progresses
            .last()
            .map(|progress| progress.UpdateTime)
            .unwrap_or(SystemTime::UNIX_EPOCH)
    }

    /// 清零计数与历史，并写入一个零值采样点。
    /// Reset clears all counters and history, then records a zero-value sample.
    pub fn Reset(&mut self) {
        self.RowCnt.store(0, Ordering::SeqCst);
        self.Processed.store(0, Ordering::SeqCst);
        self.ReadBytes.store(0, Ordering::SeqCst);
        self.PutReqCnt.store(0, Ordering::SeqCst);
        self.GetReqCnt.store(0, Ordering::SeqCst);
        self.Progresses.clear();
        self.Update();
    }
}

/// 运行中子任务的指标收集器。
/// Collector collects metrics for a running subtask.
pub trait Collector {
    fn Accepted(&self, bytes: i64);
    fn Processed(&self, processed_units: i64, rows: i64);
}

#[derive(Debug, Default)]
/// 空实现收集器，丢弃所有指标。
pub struct NoopCollector;

impl Collector for NoopCollector {
    fn Accepted(&self, _bytes: i64) {}
    fn Processed(&self, _processed_units: i64, _rows: i64) {}
}

/// 测试用收集器，原子累加 Accepted/Processed 调用。
/// TestCollector records all Collector calls atomically.
#[derive(Debug, Default)]
pub struct TestCollector {
    pub NoopCollector: NoopCollector,
    pub ReadBytes: AtomicI64,
    pub ProcessedCnt: AtomicI64,
    pub Rows: AtomicI64,
}

impl Collector for TestCollector {
    fn Accepted(&self, bytes: i64) {
        self.ReadBytes.fetch_add(bytes, Ordering::SeqCst);
    }

    fn Processed(&self, processed_units: i64, rows: i64) {
        self.ProcessedCnt
            .fetch_add(processed_units, Ordering::SeqCst);
        self.Rows.fetch_add(rows, Ordering::SeqCst);
    }
}

/// 更新子任务 checkpoint 的回调类型。
pub type CheckpointUpdateFunc =
    Arc<dyn Fn(Context, i64, Box<dyn Any + Send + Sync>) -> Result<()> + Send + Sync + 'static>;
/// 读取子任务 checkpoint 的回调类型。
pub type CheckpointGetFunc = Arc<dyn Fn(Context, i64) -> Result<String> + Send + Sync + 'static>;

/// 向 StepExecutor 暴露框架持有的 step/资源/计量/checkpoint 状态。
/// StepExecFrameworkInfo exposes framework-owned state to a StepExecutor.
pub trait StepExecFrameworkInfo {
    fn restricted(&self);
    fn GetStep(&self) -> Step;
    fn GetResource(&self) -> Option<Arc<StepResource>>;
    fn SetResource(&self, resource: Arc<StepResource>);
    fn GetMeterRecorder(&self) -> Option<Arc<metering::Recorder>>;
    fn GetCheckpointUpdateFunc(&self) -> Option<CheckpointUpdateFunc>;
    fn GetCheckpointFunc(&self) -> Option<CheckpointGetFunc>;
}

/// 框架信息类型名（与 Go 反射/注册名对齐）。
pub const stepExecFrameworkInfoName: &str = "StepExecFrameworkInfo";

/// Go embedding 的 frameworkInfo 在 Rust 中的显式结构。
/// FrameworkInfo is the explicit Rust equivalent of Go's embedded frameworkInfo.
pub struct FrameworkInfo {
    step: Step,
    meter_recorder: Arc<metering::Recorder>,
    resource: RwLock<Option<Arc<StepResource>>>,
    update_checkpoint_func: Option<CheckpointUpdateFunc>,
    get_checkpoint_func: Option<CheckpointGetFunc>,
}

impl FrameworkInfo {
    /// 从任务与资源构造 FrameworkInfo，并注册计量 Recorder。
    fn new(
        task: &Task,
        resource: Arc<StepResource>,
        update_checkpoint_func: Option<CheckpointUpdateFunc>,
        get_checkpoint_func: Option<CheckpointGetFunc>,
    ) -> Self {
        Self {
            step: task.TaskBase.Step,
            meter_recorder: metering::RegisterRecorder(&task.TaskBase),
            resource: RwLock::new(Some(resource)),
            update_checkpoint_func,
            get_checkpoint_func,
        }
    }
}

impl StepExecFrameworkInfo for FrameworkInfo {
    fn restricted(&self) {}

    fn GetStep(&self) -> Step {
        self.step
    }

    fn GetResource(&self) -> Option<Arc<StepResource>> {
        self.resource
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn SetResource(&self, resource: Arc<StepResource>) {
        *self
            .resource
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(resource);
    }

    fn GetMeterRecorder(&self) -> Option<Arc<metering::Recorder>> {
        Some(Arc::clone(&self.meter_recorder))
    }

    fn GetCheckpointUpdateFunc(&self) -> Option<CheckpointUpdateFunc> {
        self.update_checkpoint_func.clone()
    }

    fn GetCheckpointFunc(&self) -> Option<CheckpointGetFunc> {
        self.get_checkpoint_func.clone()
    }
}

/// 向执行器注入框架状态；exec 为 None 时为 no-op。
/// SetFrameworkInfo injects framework state into an executor, matching the Go helper.
pub fn SetFrameworkInfo(
    exec: Option<&mut dyn StepExecutor>,
    task: &Task,
    resource: Arc<StepResource>,
    update_checkpoint_func: Option<CheckpointUpdateFunc>,
    get_checkpoint_func: Option<CheckpointGetFunc>,
) {
    let Some(exec) = exec else {
        return;
    };
    exec.SetFrameworkInfo(FrameworkInfo::new(
        task,
        resource,
        update_checkpoint_func,
        get_checkpoint_func,
    ));
}
