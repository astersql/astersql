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

// DXF（Distributed eXecution Framework，分布式执行框架）指标采集器的迁移对齐单元测试。
//
// 验证 Collector 按任务/子任务维度聚合 Prometheus 指标，以及 InitDistTaskMetrics
// 注册的指标名、标签与 Go 侧保持一致。

use astersql_dxf_framework_dxfmetric::*;
use prometheus::{Encoder, Registry, TextEncoder};
use proto::step::StepInit;
use proto::subtask::{SubtaskBase, SubtaskStateFailed, SubtaskStatePending, SubtaskStateRunning};
use proto::task::{ExtraParams, TaskBase, TaskStateFailed, TaskStatePending, TaskStateRunning};
use proto::r#type::{Backfill, ImportInto};
use std::time::{Duration, SystemTime};

/// 构造最小可用的任务基座（TaskBase），供指标聚合测试注入状态快照。
fn task(id: i64, task_type: &'static str, state: &'static str) -> TaskBase {
    TaskBase {
        ID: id,
        Key: format!("task-{id}"),
        Type: task_type,
        State: state,
        Step: StepInit,
        Priority: 512,
        RequiredSlots: 1,
        TargetScope: String::new(),
        CreateTime: SystemTime::UNIX_EPOCH,
        MaxNodeCount: 0,
        ExtraParams: ExtraParams::default(),
        Keyspace: String::new(),
    }
}

/// 构造子任务（Subtask）；`age` 用于推算 CreateTime/StartTime，从而验证 duration 指标。
fn subtask(
    id: i64,
    task_id: i64,
    task_type: &'static str,
    state: &'static str,
    exec_id: &str,
    age: Duration,
) -> SubtaskBase {
    let timestamp = SystemTime::now() - age;
    SubtaskBase {
        ID: id,
        Step: StepInit,
        Type: task_type,
        TaskID: task_id,
        State: state,
        Concurrency: 1,
        ExecID: exec_id.to_owned(),
        CreateTime: timestamp,
        StartTime: timestamp,
        Ordinal: 1,
    }
}

/// 将 Registry 中已注册指标编码为 Prometheus 文本格式，便于断言标签与数值。
fn gather_text(registry: &Registry) -> String {
    let mut output = Vec::new();
    TextEncoder::new()
        .encode(&registry.gather(), &mut output)
        .unwrap();
    String::from_utf8(output).unwrap()
}

#[test]
/// 验证 Collector 按 task_type/status/exec_id 等维度聚合计数，并输出子任务 duration；
/// Failed 子任务不应出现在 duration 指标中（与 Go 行为对齐）。
fn migration_collector_aggregates_go_dimensions_and_durations() {
    let collector = Collector::new(false);
    collector.UpdateInfo(
        vec![
            task(1, Backfill, TaskStatePending),
            task(2, Backfill, TaskStatePending),
            task(3, ImportInto, TaskStateRunning),
            task(4, ImportInto, TaskStateFailed),
        ],
        vec![
            subtask(
                10,
                1,
                Backfill,
                SubtaskStatePending,
                "node-a",
                Duration::from_secs(5),
            ),
            subtask(
                11,
                1,
                Backfill,
                SubtaskStatePending,
                "node-a",
                Duration::from_secs(7),
            ),
            subtask(
                12,
                1,
                Backfill,
                SubtaskStateRunning,
                "node-b",
                Duration::from_secs(3),
            ),
            subtask(
                13,
                1,
                Backfill,
                SubtaskStateFailed,
                "node-b",
                Duration::from_secs(9),
            ),
        ],
    );

    let registry = Registry::new();
    registry.register(Box::new(collector)).unwrap();
    let text = gather_text(&registry);

    assert!(
        text.contains("tidb_disttask_task_status{status=\"pending\",task_type=\"backfill\"} 2")
    );
    assert!(text.contains("tidb_disttask_subtasks{exec_id=\"node-a\",status=\"pending\",task_id=\"1\",task_type=\"backfill\"} 2"));
    assert!(text.contains("tidb_disttask_subtasks{exec_id=\"node-b\",status=\"failed\",task_id=\"1\",task_type=\"backfill\"} 1"));
    assert!(text.contains("tidb_disttask_subtask_duration{exec_id=\"node-a\",status=\"pending\",subtask_id=\"10\",task_id=\"1\",task_type=\"backfill\"}"));
    assert!(text.contains("tidb_disttask_subtask_duration{exec_id=\"node-b\",status=\"running\",subtask_id=\"12\",task_id=\"1\",task_type=\"backfill\"}"));
    assert!(!text.contains("subtask_id=\"13\""));
}

#[test]
/// 连续两次 UpdateInfo 后，旧快照应被完整替换，不应残留上一批任务类型标签。
fn migration_collector_replaces_snapshot_atomically() {
    let collector = Collector::new(false);
    collector.UpdateInfo(vec![task(1, Backfill, TaskStatePending)], Vec::new());
    collector.UpdateInfo(vec![task(2, ImportInto, TaskStateRunning)], Vec::new());

    let registry = Registry::new();
    registry.register(Box::new(collector)).unwrap();
    let text = gather_text(&registry);
    assert!(!text.contains("task_type=\"backfill\""));
    assert!(text.contains("status=\"running\",task_type=\"ImportInto\"} 1"));
}

#[test]
/// 写入 DistTask 相关 Gauge/Counter 后，Register 产出的指标名与标签应与 Go 一致。
fn migration_metric_names_labels_and_registration_match_go() {
    let metrics = InitDistTaskMetrics();
    metrics
        .UsedSlotsGauge
        .with_label_values(&["background"])
        .set(2.0);
    metrics.WorkerCount.with_label_values(&["import"]).set(3.0);
    metrics
        .FinishedTaskCounter
        .with_label_values(&["succeed"])
        .inc();
    metrics
        .ScheduleEventCounter
        .with_label_values(&["42", EventRetry])
        .inc();
    metrics
        .ExecuteEventCounter
        .with_label_values(&["42", EventSubtaskSlow])
        .inc();

    let registry = Registry::new();
    Register(&registry).unwrap();
    let text = gather_text(&registry);
    assert!(text.contains("tidb_disttask_used_slots{service_scope=\"background\"} 2"));
    assert!(text.contains("tidb_dxf_worker_count{type=\"import\"} 3"));
    assert!(text.contains("tidb_dxf_finished_task_total{state=\"succeed\"} 1"));
    assert!(text.contains("tidb_dxf_schedule_event_total{event=\"retry\",task_id=\"42\"} 1"));
    assert!(text.contains("tidb_dxf_execute_event_total{event=\"subtask-slow\",task_id=\"42\"} 1"));
}
