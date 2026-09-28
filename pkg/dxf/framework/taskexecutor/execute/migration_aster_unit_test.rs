// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Aster 迁移对照测试：摘要、收集器与 FrameworkInfo 注入行为对齐 Go。
//
// 覆盖 GetSpeedInTimeRange 全用例、Update/Reset 采样窗口、对象存储
// 请求合并、Collector 原子计数，以及 SetFrameworkInfo 注入与 nil no-op。

#![allow(non_snake_case)]

use metering;
use proto;

use crate::{
    CheckpointGetFunc, CheckpointUpdateFunc, Collector, Context, FrameworkInfo, NoopCollector,
    Progress, SetFrameworkInfo, StepExecFrameworkInfo, StepExecutor, SubtaskSummary, TestCollector,
};
use http::{Method, Request};
use proto::modify::ModifyParam;
use proto::subtask::{NewAllocatable, StepResource, Subtask};
use proto::task::{ExtraParams, Task, TaskBase, TaskStatePending};
use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

/// 由秒+纳秒构造采样时间。
fn at(seconds: u64, nanos: u32) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::new(seconds, nanos)
}

/// 用 (Processed, sec, nsec) 列表构造带 Progresses 的摘要。
fn summary_with(points: &[(i64, u64, u32)]) -> SubtaskSummary {
    SubtaskSummary {
        Progresses: points
            .iter()
            .map(|(processed, seconds, nanos)| Progress {
                RowCnt: 0,
                Processed: *processed,
                UpdateTime: at(*seconds, *nanos),
            })
            .collect(),
        ..SubtaskSummary::default()
    }
}

#[test]
/// 与 Go 全套速度用例期望值逐条对照。
fn subtask_summary_speed_matches_all_go_cases() {
    let cases = [
        (
            vec![(100, 1000, 0)],
            at(1010, 0),
            Duration::from_secs(10),
            0,
        ),
        (
            vec![(0, 1000, 0), (100, 1001, 0)],
            at(1010, 0),
            Duration::from_secs(1),
            0,
        ),
        (
            vec![(0, 1000, 0), (50, 1001, 0), (100, 1002, 0), (150, 1003, 0)],
            at(1002, 500_000_000),
            Duration::from_secs(1),
            50,
        ),
        (
            vec![(0, 1000, 0), (30, 1001, 0), (60, 1002, 0), (90, 1003, 0)],
            at(1004, 0),
            Duration::from_millis(1500),
            10,
        ),
        (
            vec![
                (0, 1000, 0),
                (60, 1001, 0),
                (120, 1002, 0),
                (180, 1003, 0),
                (240, 1004, 0),
            ],
            at(1004, 500_000_000),
            Duration::from_secs(2),
            45,
        ),
        (
            vec![
                (0, 1000, 0),
                (60, 1001, 0),
                (120, 1002, 0),
                (180, 1003, 0),
                (240, 1004, 0),
            ],
            at(1004, 0),
            Duration::from_secs(4),
            60,
        ),
        (
            vec![
                (0, 1001, 0),
                (60, 1002, 0),
                (120, 1003, 0),
                (180, 1004, 0),
                (240, 1005, 0),
            ],
            at(1006, 500_000_000),
            Duration::from_secs(6),
            40,
        ),
    ];

    for (points, end, duration, expected) in cases {
        assert_eq!(
            summary_with(&points).GetSpeedInTimeRange(end, duration),
            expected
        );
    }
}

#[test]
/// Go int64 的采样差值按二补码回绕，Rust 调试构建也必须保持该语义。
fn subtask_summary_speed_wraps_processed_delta_like_go() {
    let summary = summary_with(&[(i64::MAX, 1000, 0), (i64::MIN, 1001, 0)]);

    assert_eq!(
        summary.GetSpeedInTimeRange(at(1001, 0), Duration::from_secs(1)),
        1
    );
}

#[test]
/// Update 只保留最近 5 点；Reset 清零后留下一个零值采样。
fn update_retains_five_latest_points_and_reset_matches_go() {
    let mut summary = SubtaskSummary::default();
    for value in 1..=7 {
        summary.RowCnt.store(value, Ordering::SeqCst);
        summary.Processed.store(value * 10, Ordering::SeqCst);
        summary.Update();
    }
    assert_eq!(summary.Progresses.len(), 5);
    assert_eq!(summary.Progresses[0].Processed, 30);
    assert_eq!(summary.Progresses[4].Processed, 70);

    summary.ReadBytes.store(11, Ordering::SeqCst);
    summary.GetReqCnt.store(12, Ordering::SeqCst);
    summary.PutReqCnt.store(13, Ordering::SeqCst);
    summary.Reset();
    assert_eq!(summary.RowCnt.load(Ordering::SeqCst), 0);
    assert_eq!(summary.Processed.load(Ordering::SeqCst), 0);
    assert_eq!(summary.ReadBytes.load(Ordering::SeqCst), 0);
    assert_eq!(summary.GetReqCnt.load(Ordering::SeqCst), 0);
    assert_eq!(summary.PutReqCnt.load(Ordering::SeqCst), 0);
    assert_eq!(summary.Progresses.len(), 1);
    assert_eq!(summary.UpdateTime(), summary.Progresses[0].UpdateTime);
}

#[test]
/// 对象存储 GET/HEAD/PUT 合并计数；Noop/TestCollector 行为。
fn request_merge_and_collectors_preserve_atomic_counts() {
    let stats = recording::AccessStats::default();
    for method in [Method::GET, Method::HEAD, Method::PUT] {
        let request = Request::builder().method(method).body(()).unwrap();
        recording::AccessStats::rec_request(Some(&stats), Some(&request));
    }

    let summary = SubtaskSummary::default();
    summary.MergeObjStoreRequests(&stats.requests);
    assert_eq!(summary.GetReqCnt.load(Ordering::SeqCst), 2);
    assert_eq!(summary.PutReqCnt.load(Ordering::SeqCst), 1);

    let noop = NoopCollector;
    noop.Accepted(99);
    noop.Processed(88, 77);

    let collector = TestCollector::default();
    collector.Accepted(5);
    collector.Processed(8, 13);
    assert_eq!(collector.ReadBytes.load(Ordering::SeqCst), 5);
    assert_eq!(collector.ProcessedCnt.load(Ordering::SeqCst), 8);
    assert_eq!(collector.Rows.load(Ordering::SeqCst), 13);
}

/// 测试用 StepExecutor：仅持有并转发 FrameworkInfo。
struct TestExecutor {
    framework: Option<FrameworkInfo>,
}

impl StepExecFrameworkInfo for TestExecutor {
    fn restricted(&self) {}

    fn GetStep(&self) -> proto::step::Step {
        self.framework.as_ref().unwrap().GetStep()
    }

    fn GetResource(&self) -> Option<Arc<StepResource>> {
        self.framework.as_ref().unwrap().GetResource()
    }

    fn SetResource(&self, resource: Arc<StepResource>) {
        self.framework.as_ref().unwrap().SetResource(resource);
    }

    fn GetMeterRecorder(&self) -> Option<Arc<metering::Recorder>> {
        self.framework.as_ref().unwrap().GetMeterRecorder()
    }

    fn GetCheckpointUpdateFunc(&self) -> Option<CheckpointUpdateFunc> {
        self.framework.as_ref().unwrap().GetCheckpointUpdateFunc()
    }

    fn GetCheckpointFunc(&self) -> Option<CheckpointGetFunc> {
        self.framework.as_ref().unwrap().GetCheckpointFunc()
    }
}

impl StepExecutor for TestExecutor {
    fn Init(&mut self, _ctx: Context) -> anyhow::Result<()> {
        Ok(())
    }
    fn RunSubtask(&mut self, _ctx: Context, _subtask: &mut Subtask) -> anyhow::Result<()> {
        Ok(())
    }
    fn RealtimeSummary(&mut self) -> Option<&SubtaskSummary> {
        None
    }
    fn ResetSummary(&mut self) {}
    fn Cleanup(&mut self, _ctx: Context) -> anyhow::Result<()> {
        Ok(())
    }
    fn TaskMetaModified(&mut self, _ctx: Context, _new_meta: Vec<u8>) -> anyhow::Result<()> {
        Ok(())
    }
    fn ResourceModified(
        &mut self,
        _ctx: Context,
        _new_resource: &StepResource,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn SetFrameworkInfo(&mut self, info: FrameworkInfo) {
        self.framework = Some(info);
    }
}

/// 构造注入 FrameworkInfo 用的最小 Task。
fn task(step: i64) -> Task {
    Task {
        TaskBase: TaskBase {
            ID: 42,
            Key: "task-42".to_string(),
            Type: "example",
            State: TaskStatePending,
            Step: step,
            Priority: 512,
            RequiredSlots: 4,
            TargetScope: String::new(),
            CreateTime: SystemTime::UNIX_EPOCH,
            MaxNodeCount: 0,
            ExtraParams: ExtraParams::default(),
            Keyspace: "ks".to_string(),
        },
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: Vec::new(),
        Error: None,
        ModifyParam: ModifyParam {
            PrevState: TaskStatePending,
            Modifications: Vec::new(),
        },
    }
}

#[test]
/// 校验 step/资源/计量/checkpoint 回调均可经 FrameworkInfo 访问。
fn set_framework_info_injects_all_go_visible_state() {
    let mut executor = TestExecutor { framework: None };
    let resource = Arc::new(StepResource {
        CPU: NewAllocatable(4),
        Mem: NewAllocatable(1024),
    });
    let update: CheckpointUpdateFunc = Arc::new(
        |_ctx: Context, subtask_id: i64, value: Box<dyn Any + Send + Sync>| {
            assert_eq!(subtask_id, 7);
            assert_eq!(*value.downcast::<u64>().unwrap(), 9);
            Ok(())
        },
    );
    let get: CheckpointGetFunc =
        Arc::new(|_ctx: Context, subtask_id: i64| Ok(format!("checkpoint-{subtask_id}")));

    SetFrameworkInfo(
        Some(&mut executor),
        &task(3),
        Arc::clone(&resource),
        Some(update),
        Some(get),
    );

    assert_eq!(executor.GetStep(), 3);
    assert_eq!(executor.GetResource().unwrap().CPU.Capacity(), 4);
    assert!(executor.GetMeterRecorder().is_some());
    executor.GetCheckpointUpdateFunc().unwrap()(Context::new(), 7, Box::new(9_u64)).unwrap();
    assert_eq!(
        executor.GetCheckpointFunc().unwrap()(Context::new(), 7).unwrap(),
        "checkpoint-7"
    );

    let replacement = Arc::new(StepResource {
        CPU: NewAllocatable(2),
        Mem: NewAllocatable(512),
    });
    executor.SetResource(replacement);
    assert_eq!(executor.GetResource().unwrap().CPU.Capacity(), 2);
}

#[test]
/// exec 为 None 时 SetFrameworkInfo 应直接返回。
fn nil_executor_is_a_noop() {
    let resource = Arc::new(StepResource {
        CPU: NewAllocatable(1),
        Mem: NewAllocatable(1),
    });
    SetFrameworkInfo(None, &task(1), resource, None, None);
}
