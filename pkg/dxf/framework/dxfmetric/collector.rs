// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// DXF 自定义 Prometheus Collector。
//
// 缓存任务/子任务快照，在 scrape 时按类型与状态聚合为 Gauge，
// 并输出 pending/running 子任务已持续时长。

use prometheus::core::{Collector as PrometheusCollector, Desc};
use prometheus::proto::MetricFamily;
use prometheus::{GaugeVec, Opts};
use proto::subtask::{SubtaskBase, SubtaskStatePending, SubtaskStateRunning};
use proto::task::{TaskBase, TaskType};
use std::collections::HashMap;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::SystemTime;

/// 常量标签键值（如测试用的 server_id）。
type Labels = HashMap<String, String>;

/// Custom Prometheus collector for DXF task and subtask snapshots.
/// DXF 任务与子任务快照的自定义 Prometheus 采集器。
pub struct Collector {
    /// 任务与子任务快照（读写锁保护）。
    snapshot: RwLock<Snapshot>,
    /// 预声明的 Prometheus 描述符。
    descriptors: Vec<Desc>,
    /// 常量标签（测试时含唯一 server_id）。
    constLabels: Labels,
}

/// 一次 UpdateInfo 写入的任务与子任务快照。
struct Snapshot {
    /// 任务基础信息列表。
    tasks: Vec<TaskBase>,
    /// 子任务基础信息列表。
    subtasks: Vec<SubtaskBase>,
}

/// 读锁；中毒时仍取内层数据，避免指标采集永久失败。
fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 写锁；中毒时同样恢复内层数据。
fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 将 &str 切片转为拥有所有权的 String 列表。
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// 构造 Prometheus Desc；非法定义直接 panic。
fn descriptor(name: &str, help: &str, labels: &[&str], const_labels: Labels) -> Desc {
    metricscommon::NewDesc(name, help, &strings(labels), const_labels)
        .expect("DXF metric descriptor must be valid")
}

/// Collector 构造、更新快照与指标族收集。
impl Collector {
    /// Creates a collector. Tests may request a unique server label, matching Go's intest path.
    /// 创建采集器；测试模式下附加唯一 server_id 标签（对齐 Go intest）。
    pub fn new(in_test: bool) -> Self {
        let mut const_labels = Labels::new();
        if in_test {
            const_labels.insert("server_id".to_owned(), uuid::Uuid::new_v4().to_string());
        }

        Self {
            snapshot: RwLock::new(Snapshot {
                tasks: Vec::new(),
                subtasks: Vec::new(),
            }),
            descriptors: vec![
                descriptor(
                    "tidb_disttask_task_status",
                    "Number of tasks.",
                    &["task_type", "status"],
                    const_labels.clone(),
                ),
                descriptor(
                    "tidb_disttask_subtasks",
                    "Number of subtasks.",
                    &["task_type", "task_id", "status", "exec_id"],
                    const_labels.clone(),
                ),
                descriptor(
                    "tidb_disttask_subtask_duration",
                    "Duration of subtasks in different states.",
                    &["task_type", "task_id", "status", "subtask_id", "exec_id"],
                    const_labels.clone(),
                ),
            ],
            constLabels: const_labels,
        }
    }

    /// Atomically replaces both snapshots from a scraper's perspective.
    /// 原子替换任务与子任务快照，供 scrape 读取。
    pub fn UpdateInfo(&self, tasks: Vec<TaskBase>, subtasks: Vec<SubtaskBase>) {
        *write_lock(&self.snapshot) = Snapshot { tasks, subtasks };
    }

    /// 按本 collector 的 constLabels 新建临时 GaugeVec。
    fn gauge_vec(&self, name: &str, help: &str, labels: &[&str]) -> GaugeVec {
        let label_names = labels.to_vec();
        GaugeVec::new(
            Opts::new(name, help).const_labels(self.constLabels.clone()),
            &label_names,
        )
        .expect("DXF metric definition must be valid")
    }

    /// 按 (task_type, status) 计数任务，产出 tidb_disttask_task_status。
    fn collectTasks(&self, snapshot: &Snapshot) -> Vec<MetricFamily> {
        let tasks = self.gauge_vec(
            "tidb_disttask_task_status",
            "Number of tasks.",
            &["task_type", "status"],
        );
        let mut counts: HashMap<(String, String), u64> = HashMap::new();
        for task in &snapshot.tasks {
            *counts
                .entry((task.Type.to_string(), task.State.to_string()))
                .or_default() += 1;
        }
        for ((task_type, state), count) in counts {
            tasks
                .with_label_values(&[&task_type, &state])
                .set(count as f64);
        }
        PrometheusCollector::collect(&tasks)
    }

    /// 聚合子任务个数，并为 pending/running 计算持续时长。
    fn collectSubtasks(&self, snapshot: &Snapshot) -> Vec<MetricFamily> {
        let subtasks_metric = self.gauge_vec(
            "tidb_disttask_subtasks",
            "Number of subtasks.",
            &["task_type", "task_id", "status", "exec_id"],
        );
        let duration_metric = self.gauge_vec(
            "tidb_disttask_subtask_duration",
            "Duration of subtasks in different states.",
            &["task_type", "task_id", "status", "subtask_id", "exec_id"],
        );

        // task ID => exec ID => state => count, with task type retained per task ID.
        let mut counts: HashMap<(i64, String, &'static str), u64> = HashMap::new();
        let mut task_types: HashMap<i64, TaskType> = HashMap::new();
        for subtask in &snapshot.subtasks {
            *counts
                .entry((subtask.TaskID, subtask.ExecID.clone(), subtask.State))
                .or_default() += 1;
            task_types.insert(subtask.TaskID, subtask.Type);

            let start = match subtask.State {
                SubtaskStatePending => Some(subtask.CreateTime),
                SubtaskStateRunning => Some(subtask.StartTime),
                _ => None,
            };
            if let Some(start) = start {
                let duration = seconds_since(start);
                duration_metric
                    .with_label_values(&[
                        &subtask.Type.to_string(),
                        &subtask.TaskID.to_string(),
                        &subtask.State.to_string(),
                        &subtask.ID.to_string(),
                        &subtask.ExecID,
                    ])
                    .set(duration);
            }
        }

        for ((task_id, exec_id, state), count) in counts {
            let task_type = task_types[&task_id].to_string();
            subtasks_metric
                .with_label_values(&[
                    &task_type,
                    &task_id.to_string(),
                    &state.to_string(),
                    &exec_id,
                ])
                .set(count as f64);
        }

        let mut families = PrometheusCollector::collect(&subtasks_metric);
        families.extend(PrometheusCollector::collect(&duration_metric));
        families
    }
}

/// Go-compatible constructor name.
/// 对齐 Go 的 NewCollector 构造名（非测试模式）。
pub fn NewCollector() -> Collector {
    Collector::new(false)
}

/// 距 now 的秒数；若 then 在未来则返回负值（时钟回拨场景）。
fn seconds_since(then: SystemTime) -> f64 {
    match SystemTime::now().duration_since(then) {
        Ok(duration) => duration.as_secs_f64(),
        Err(error) => -error.duration().as_secs_f64(),
    }
}

/// 实现 prometheus::Collector：暴露 Desc 与 collect。
impl PrometheusCollector for Collector {
    /// 返回预注册的指标描述符。
    fn desc(&self) -> Vec<&Desc> {
        self.descriptors.iter().collect()
    }

    /// 读快照并合并任务与子任务指标族。
    fn collect(&self) -> Vec<MetricFamily> {
        let snapshot = read_lock(&self.snapshot);
        let mut families = self.collectTasks(&snapshot);
        families.extend(self.collectSubtasks(&snapshot));
        families
    }
}
