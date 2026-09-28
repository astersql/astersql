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

// DXF DistTask 包级 Prometheus 指标定义。
//
// 对齐 Go `InitDistTaskMetrics`：槽位使用量、worker 数、完成任务计数、
// 调度/执行事件计数；经 OnceLock 单次初始化后可供 Register 注册。

use prometheus::{CounterVec, GaugeVec, Opts, Registry};
use std::sync::OnceLock;

/// Prometheus namespace：tidb。
const namespaceTiDB: &str = "tidb";
/// 多数 DXF 指标的 subsystem：dxf。
const subsystemDXF: &str = "dxf";
/// 标签名：类型（如 worker 类型）。
const lblType: &str = "type";
/// 标签名：事件名。
const lblEvent: &str = "event";
/// 标签名：任务终态等。
const lblState: &str = "state";

/// 公开标签名：任务 ID（控制事件指标基数）。
pub const LblTaskID: &str = "task_id";

/// 事件：子任务被调度到其他节点。
pub const EventSubtaskScheduledAway: &str = "subtask-scheduled-away";
/// 事件：子任务重跑。
pub const EventSubtaskRerun: &str = "subtask-rerun";
/// 事件：子任务执行过慢。
pub const EventSubtaskSlow: &str = "subtask-slow";
/// 事件：重试。
pub const EventRetry: &str = "retry";
/// 事件：索引过多相关。
pub const EventTooManyIdx: &str = "too-many-idx";
/// 事件：归并排序相关。
pub const EventMergeSort: &str = "merge-sort";
/// 事件：清理失败。
pub const EventCleanupFailed: &str = "cleanup-failed";
/// 事件：计量写入失败。
pub const EventMeterWriteFailed: &str = "meter-write-failed";

/// The five package-level metric vectors initialized by Go's InitDistTaskMetrics.
/// Go InitDistTaskMetrics 初始化的五组包级指标向量。
pub struct DistTaskMetrics {
    /// 执行节点已用槽位数（按 service_scope）。
    pub UsedSlotsGauge: GaugeVec,
    /// DXF worker 数量（按 type）。
    pub WorkerCount: GaugeVec,
    /// 已完成任务计数（按 state）。
    pub FinishedTaskCounter: CounterVec,
    /// 调度侧事件计数（task_id + event）。
    pub ScheduleEventCounter: CounterVec,
    /// 执行侧事件计数（用 task_id 限制基数，而非 subtask_id）。
    pub ExecuteEventCounter: CounterVec,
}

/// 进程内单例 DistTaskMetrics。
static METRICS: OnceLock<DistTaskMetrics> = OnceLock::new();

/// 构造带 namespace/subsystem 的 Opts。
fn opts(namespace: &str, subsystem: &str, name: &str, help: &str) -> Opts {
    Opts::new(name, help)
        .namespace(namespace)
        .subsystem(subsystem)
}

/// 标签名列表转为 String。
fn labels(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Initializes DXF metrics once and returns the shared vectors.
/// 单次初始化 DXF 指标并返回共享向量。
pub fn InitDistTaskMetrics() -> &'static DistTaskMetrics {
    METRICS.get_or_init(|| DistTaskMetrics {
        UsedSlotsGauge: metricscommon::NewGaugeVec(
            opts(
                namespaceTiDB,
                "disttask",
                "used_slots",
                "Gauge of used slots on a executor node.",
            ),
            &labels(&["service_scope"]),
        ),
        WorkerCount: metricscommon::NewGaugeVec(
            opts(
                namespaceTiDB,
                subsystemDXF,
                "worker_count",
                "Gauge of DXF worker count.",
            ),
            &labels(&[lblType]),
        ),
        FinishedTaskCounter: metricscommon::NewCounterVec(
            opts(
                namespaceTiDB,
                subsystemDXF,
                "finished_task_total",
                "Counter of finished DXF tasks.",
            ),
            &labels(&[lblState]),
        ),
        ScheduleEventCounter: metricscommon::NewCounterVec(
            opts(
                namespaceTiDB,
                subsystemDXF,
                "schedule_event_total",
                "Counter of DXF schedule events fo tasks.",
            ),
            &labels(&[LblTaskID, lblEvent]),
        ),
        // Task ID is intentionally used instead of subtask ID to limit cardinality.
        ExecuteEventCounter: metricscommon::NewCounterVec(
            opts(
                namespaceTiDB,
                subsystemDXF,
                "execute_event_total",
                "Counter of DXF execute events fo tasks.",
            ),
            &labels(&[LblTaskID, lblEvent]),
        ),
    })
}

/// Registers all DXF metrics in Go declaration order.
/// 按 Go 声明顺序将全部 DXF 指标注册到 Registry。
pub fn Register(register: &Registry) -> prometheus::Result<()> {
    let metrics = InitDistTaskMetrics();
    register.register(Box::new(metrics.UsedSlotsGauge.clone()))?;
    register.register(Box::new(metrics.WorkerCount.clone()))?;
    register.register(Box::new(metrics.FinishedTaskCounter.clone()))?;
    register.register(Box::new(metrics.ScheduleEventCounter.clone()))?;
    register.register(Box::new(metrics.ExecuteEventCounter.clone()))?;
    Ok(())
}
