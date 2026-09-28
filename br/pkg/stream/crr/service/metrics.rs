// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! Prometheus gauge，镜像 Go metrics.go 的 collector、标签与注册行为。
//! 职责：把 StatusSnapshot 展开为按 task（及 state/phase）标签的标量，供观测与契约测试读取。
//! 约束：最多两个标签值编码进键；state/phase 用 one-hot 1/0，避免枚举型 gauge 丢历史。
//! 全局 OnceLock 单例，首次使用时向默认 Prometheus registry 注册全部 collector。
//! 指标名与 Go `metrics.go` 字符串保持一致，便于交叉对照仪表盘。

use std::collections::HashMap;
use std::sync::OnceLock;

use prometheus::{GaugeVec, Opts};

use astersql_br_pkg_stream_crr_internal_checkpoint::EventType;

use crate::status::{
    PHASE_IDLE, STATE_DEGRADED, STATE_RUNNING, STATE_STARTING, STATE_STOPPED, StatusSnapshot,
};

/// 指标短名 → 已注册 GaugeVec；短名仅用于包内统一分派。
struct GaugeMap {
    values: HashMap<&'static str, GaugeVec>,
}

impl GaugeMap {
    fn new() -> Self {
        let mut values = HashMap::new();
        for (name, help, labels) in [
            (
                "service_live",
                "Whether CRR status service is live (1 means true, 0 means false).",
                &["task"][..],
            ),
            (
                "service_ready",
                "Whether CRR status service is ready (1 means true, 0 means false).",
                &["task"][..],
            ),
            (
                "service_state",
                "Current CRR service state by task and state label.",
                &["task", "state"][..],
            ),
            (
                "service_phase",
                "Current CRR service phase by task and phase label.",
                &["task", "phase"][..],
            ),
            (
                "current_round",
                "Current CRR calculation round.",
                &["task"][..],
            ),
            (
                "last_loop_iteration",
                "Last CRR loop iteration to wait downstream files synced in current phase.",
                &["task"][..],
            ),
            (
                "last_upstream_checkpoint",
                "Latest upstream checkpoint observed by CRR.",
                &["task"][..],
            ),
            (
                "safe_checkpoint",
                "Current safe checkpoint(synced upstream checkpoint) of CRR.",
                &["task"][..],
            ),
            (
                "synced_ts",
                "Current synced ts(replication-complete checkpoint) of CRR.",
                &["task"][..],
            ),
            (
                "alive_store_count",
                "Alive store count in latest planning/advance event.",
                &["task"][..],
            ),
            (
                "pending_file_count",
                "Pending file count in current CRR round.",
                &["task"][..],
            ),
            (
                "consecutive_failures",
                "Consecutive failure count in CRR service.",
                &["task"][..],
            ),
            (
                "upstream_read_meta_file_count",
                "Read upstream meta file count in latest round statistic.",
                &["task"][..],
            ),
            (
                "skipped_store_synced_meta_file_count",
                "Skipped upstream meta file count because the store is already synced past the meta flush ts in latest round statistic.",
                &["task"][..],
            ),
            (
                "estimated_sync_log_file_count",
                "Estimated sync log file count in latest round statistic.",
                &["task"][..],
            ),
            (
                "downstream_check_file_count",
                "Downstream check file count in latest round statistic.",
                &["task"][..],
            ),
        ] {
            let opts = Opts::new(name, help).namespace("tidb").subsystem("br_crr");
            let gauge = GaugeVec::new(opts, labels).expect("valid CRR gauge definition");
            prometheus::default_registry()
                .register(Box::new(gauge.clone()))
                .expect("register CRR gauge");
            values.insert(name, gauge);
        }
        Self { values }
    }

    /// 写入已注册 gauge；调用点提供的标签顺序与声明顺序一致。
    fn set(&self, name: &str, labels: &[(&str, &str)], value: f64) {
        let label_values: Vec<&str> = labels.iter().map(|(_, value)| *value).collect();
        self.values
            .get(name)
            .expect("known CRR gauge")
            .with_label_values(&label_values)
            .set(value);
    }

    /// 测试读取，与 Go promtest.ToFloat64(Gauge) 对齐。
    fn get(&self, name: &str, labels: &[(&str, &str)]) -> f64 {
        let label_values: Vec<&str> = labels.iter().map(|(_, value)| *value).collect();
        self.values
            .get(name)
            .expect("known CRR gauge")
            .with_label_values(&label_values)
            .get()
    }
}

/// 进程级单例；首次观察时惰性初始化。
fn metrics() -> &'static GaugeMap {
    static METRICS: OnceLock<GaugeMap> = OnceLock::new();
    METRICS.get_or_init(GaugeMap::new)
}

/// 测试辅助：读取“已按 store 同步而跳过的 meta”计数。
pub(crate) fn skipped_store_synced_meta_file_count_metric(task: &str) -> f64 {
    metrics().get("skipped_store_synced_meta_file_count", &[("task", task)])
}

/// 将完整状态快照刷入各 gauge；应在每次状态变更后调用。
/// state/phase 全量枚举写 0/1，保证旧相位 gauge 被清零。
pub(crate) fn observe_status_metrics(snapshot: &StatusSnapshot) {
    let m = metrics();
    // 服务生命周期状态 one-hot。
    for state in [STATE_STARTING, STATE_RUNNING, STATE_DEGRADED, STATE_STOPPED] {
        let value = if state == snapshot.State { 1.0 } else { 0.0 };
        m.set(
            "service_state",
            &[("task", &snapshot.TaskName), ("state", state)],
            value,
        );
    }

    // 计算相位 one-hot；字符串与 EventType/PHASE_IDLE 对齐 Go 侧 label。
    for phase in [
        PHASE_IDLE,
        EventType::EventWaitingUpstream.as_str(),
        EventType::EventUpstreamAdvanced.as_str(),
        EventType::EventRoundPlanned.as_str(),
        EventType::EventWaitingDownstream.as_str(),
        EventType::EventCheckpointAdvanced.as_str(),
        EventType::EventCalculationFailed.as_str(),
    ] {
        let value = if phase == snapshot.Phase { 1.0 } else { 0.0 };
        m.set(
            "service_phase",
            &[("task", &snapshot.TaskName), ("phase", phase)],
            value,
        );
    }

    // 布尔探针：与 /livez /readyz 语义同源。
    m.set(
        "service_live",
        &[("task", &snapshot.TaskName)],
        bool_to_float(snapshot.Live),
    );
    m.set(
        "service_ready",
        &[("task", &snapshot.TaskName)],
        bool_to_float(snapshot.Ready),
    );
    // 进度类标量：轮次、迭代、上下游水位。
    // 当前计算轮次（自服务启动累计）。
    m.set(
        "current_round",
        &[("task", &snapshot.TaskName)],
        snapshot.CurrentRound as f64,
    );
    // 最近一次等待循环迭代号（下游或上游探测）。
    m.set(
        "last_loop_iteration",
        &[("task", &snapshot.TaskName)],
        snapshot.LastLoopIteration as f64,
    );
    // PD 上报的上游全局检查点。
    m.set(
        "last_upstream_checkpoint",
        &[("task", &snapshot.TaskName)],
        snapshot.LastUpstreamCheckpoint as f64,
    );
    // 已对下游安全的检查点（可对外承诺）。
    m.set(
        "safe_checkpoint",
        &[("task", &snapshot.TaskName)],
        snapshot.SafeCheckpoint as f64,
    );
    // 计算器内部全局 synced_ts 水位。
    m.set(
        "synced_ts",
        &[("task", &snapshot.TaskName)],
        snapshot.SyncedTS as f64,
    );
    // 最近一轮观察到的存活 store 数。
    m.set(
        "alive_store_count",
        &[("task", &snapshot.TaskName)],
        snapshot.AliveStoreCount as f64,
    );
    // 仍待下游确认的文件数。
    m.set(
        "pending_file_count",
        &[("task", &snapshot.TaskName)],
        snapshot.PendingFileCount as f64,
    );
    // 连续计算失败次数；成功推进时由状态层清零。
    m.set(
        "consecutive_failures",
        &[("task", &snapshot.TaskName)],
        snapshot.ConsecutiveFailures as f64,
    );
    // FileStatistic 子集：与 calculator 观察字段同名，便于跨层对照。
    m.set(
        "upstream_read_meta_file_count",
        &[("task", &snapshot.TaskName)],
        snapshot.Statistic.UpstreamReadMetaFileCount as f64,
    );
    // 因 store 水位已覆盖而跳过的 meta 数。
    m.set(
        "skipped_store_synced_meta_file_count",
        &[("task", &snapshot.TaskName)],
        snapshot.Statistic.SkippedStoreSyncedMetaFileCount as f64,
    );
    // 预估需同步的日志文件数（去重后）。
    m.set(
        "estimated_sync_log_file_count",
        &[("task", &snapshot.TaskName)],
        snapshot.Statistic.EstimatedSyncLogFileCount as f64,
    );
    // 下游存在性检查累计次数。
    m.set(
        "downstream_check_file_count",
        &[("task", &snapshot.TaskName)],
        snapshot.Statistic.DownstreamCheckFileCount as f64,
    );
}

/// Prometheus 风格：true→1.0 / false→0.0。
fn bool_to_float(value: bool) -> f64 {
    if value { 1.0 } else { 0.0 }
}
