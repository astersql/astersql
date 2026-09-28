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

//! CRR 服务运行时状态存储与 JSON 编码。
//! 对应 Go `br/pkg/stream/crr/service/status.go`：`StatusStore` 聚合 calculator 事件，
//! 供 `/livez`/`/readyz`/`/status` 与 Prometheus 指标读取。
//! 状态机：starting → running ↔ degraded → stopped；Phase 反映最近事件类型字符串。
//! JSON 手写编码以稳定字段顺序与空 map 省略规则，对齐 Go encoding/json 输出习惯。
//! `StatusObserver` 实现 checkpoint `Observer`，使 calculator 无需依赖 HTTP 层即可更新状态。
//! 写路径持 `RwLock` 写锁并在每次变更后调用 `observe_status_metrics`，保证指标与快照一致。
//! 读路径 `snapshot_copy` 返回深拷贝，调用方不会阻塞后续事件写入。
//! 时间字段统一为 RFC3339 UTC 秒精度；`civil_from_days` 避免引入额外 chrono 依赖。
//! 失败事件刻意不覆盖 `AliveStoreCount`，便于排障时对照“上一轮规划的 store 数”。

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use astersql_br_pkg_stream_crr_internal_checkpoint::{
    CheckpointEvent, Error, EventType, FileStatistic, PersistentState,
};

use crate::metrics::observe_status_metrics;

/// 服务刚创建、尚未调用 `start`。
pub const STATE_STARTING: &str = "starting";
/// 正常工作；`Ready=true`。
pub const STATE_RUNNING: &str = "running";
/// 连续失败后降级；`Ready=false`，等待恢复。
pub const STATE_DEGRADED: &str = "degraded";
/// `Run` 退出后终态；不再接受 clear_failure 拉回 running。
pub const STATE_STOPPED: &str = "stopped";

/// 轮次间隙或尚未收到具体阶段事件时的 Phase 缺省值。
pub const PHASE_IDLE: &str = "idle";

/// resume state 在对象存储上的相对路径；对外通过 `GetStatusFileName` 暴露以保持稳定。
const STATUS_FILE_NAME: &str = "crr-checkpoint/resume-state.json";

/// 返回 resume 状态文件相对路径；测试与运维脚本依赖该常量稳定。
pub fn GetStatusFileName() -> &'static str {
    STATUS_FILE_NAME
}

/// 当前轮次文件相关工作量摘要，字段名与 Go `StatusStatistic` / JSON 对齐。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusStatistic {
    /// 本轮从上游读取的 meta 文件数。
    pub UpstreamReadMetaFileCount: i32,
    /// 因 store 已同步而跳过的 meta 文件数（驱动 skipped metric）。
    pub SkippedStoreSyncedMetaFileCount: i32,
    /// 估算仍需同步的日志文件数。
    pub EstimatedSyncLogFileCount: i32,
    /// 下游存在性检查触及的文件数。
    pub DownstreamCheckFileCount: i32,
    /// 按后缀聚合的计划文件计数。
    pub PlannedFileSuffixCounts: HashMap<String, i32>,
    /// 按后缀聚合的下游检查文件计数。
    pub DownstreamCheckFileSuffixCounts: HashMap<String, i32>,
}

/// 对外可见的 CRR worker 状态快照；HTTP `/status` 与 `Service::Status` 均基于此。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub TaskName: String,
    /// 进程内服务是否仍在 `Run` 循环中。
    pub Live: bool,
    /// 是否可对外声称就绪（degraded/stopped 时为 false）。
    pub Ready: bool,
    /// 粗粒度状态机字符串：starting/running/degraded/stopped。
    pub State: String,
    /// 细粒度阶段，通常等于最近 `EventType` 的 as_str。
    pub Phase: String,
    /// 已开始的计算轮次计数（`begin_round` 递增）。
    pub CurrentRound: u64,
    /// 当前轮内 calculator 循环迭代号。
    pub LastLoopIteration: u64,
    /// 最近观察到的上游全局 checkpoint。
    pub LastUpstreamCheckpoint: u64,
    /// 已持久化/可安全用于恢复的 checkpoint。
    pub SafeCheckpoint: u64,
    /// 全局已同步时间戳水位。
    pub SyncedTS: u64,
    /// 按 TiKV store id 记录的同步水位。
    pub SyncedByStore: HashMap<u64, u64>,
    /// 最近规划/推进轮次中的存活 store 数。
    pub AliveStoreCount: i32,
    /// 当前轮仍待处理的文件数。
    pub PendingFileCount: i32,
    pub Statistic: StatusStatistic,
    /// 最近一次成功推进 checkpoint 的时间。
    pub LastSuccessTime: Option<SystemTime>,
    /// 最近失败错误文本；空串表示无错误。
    pub LastError: String,
    pub LastErrorTime: Option<SystemTime>,
    /// 连续失败次数；成功推进时清零。
    pub ConsecutiveFailures: u64,
    /// 任意事件到达时间，用于观察活跃度。
    pub LastEventTime: Option<SystemTime>,
}

/// `RwLock` 保护的内部快照容器；对外只经 `StatusStore` 方法访问。
struct StatusStoreInner {
    snapshot: StatusSnapshot,
}

/// 线程安全状态仓；写路径更新后同步刷新 metrics。
#[derive(Clone)]
pub struct StatusStore {
    inner: Arc<RwLock<StatusStoreInner>>,
}

impl StatusStore {
    /// 标记服务已进入运行循环：Live/Ready=true，State=running。
    pub fn start(&self) {
        let mut guard = self.inner.write().expect("status store lock");
        guard.snapshot.Live = true;
        guard.snapshot.Ready = true;
        guard.snapshot.State = STATE_RUNNING.to_string();
        guard.snapshot.Phase = PHASE_IDLE.to_string();
        observe_status_metrics(&guard.snapshot);
    }

    /// 服务退出：Live/Ready=false，State=stopped，后续 clear_failure 不再改状态。
    pub fn stop(&self) {
        let mut guard = self.inner.write().expect("status store lock");
        guard.snapshot.Live = false;
        guard.snapshot.Ready = false;
        guard.snapshot.State = STATE_STOPPED.to_string();
        observe_status_metrics(&guard.snapshot);
    }

    /// 用持久化 resume 刷新安全 checkpoint 与按 store 同步水位。
    pub fn set_persistent_state(&self, state: PersistentState) {
        let mut guard = self.inner.write().expect("status store lock");
        guard.snapshot.SafeCheckpoint = state.LastCheckpoint;
        guard.snapshot.SyncedTS = state.SyncedTS;
        guard.snapshot.SyncedByStore = state.SyncedByStore.clone();
        observe_status_metrics(&guard.snapshot);
    }

    /// 清除失败痕迹并回到 running；若已 stopped 则忽略，避免关机后被误拉起。
    pub fn clear_failure(&self) {
        let mut guard = self.inner.write().expect("status store lock");
        if guard.snapshot.State == STATE_STOPPED {
            return;
        }
        guard.snapshot.Ready = true;
        guard.snapshot.State = STATE_RUNNING.to_string();
        guard.snapshot.LastError.clear();
        guard.snapshot.LastErrorTime = None;
        guard.snapshot.ConsecutiveFailures = 0;
        observe_status_metrics(&guard.snapshot);
    }

    /// 开启新计算轮次：轮次号递增，重置迭代与 pending，Phase 回到 idle。
    fn begin_round(&self) -> u64 {
        let mut guard = self.inner.write().expect("status store lock");
        guard.snapshot.CurrentRound += 1;
        guard.snapshot.LastLoopIteration = 0;
        guard.snapshot.PendingFileCount = 0;
        guard.snapshot.Phase = PHASE_IDLE.to_string();
        observe_status_metrics(&guard.snapshot);
        guard.snapshot.CurrentRound
    }

    /// 将 calculator 事件合并进快照；按事件类型更新 Ready/State/失败计数。
    pub fn apply_event(&self, event: CheckpointEvent) {
        let mut guard = self.inner.write().expect("status store lock");

        guard.snapshot.LastEventTime = event.Time;
        // Phase 直接使用事件类型字符串，便于 HTTP 观察当前阶段。
        guard.snapshot.Phase = event.Type.as_str().to_string();
        guard.snapshot.LastLoopIteration = event.LoopIteration;
        // 零值视为“本事件未携带”，避免覆盖既有水位。
        if event.UpstreamCheckpoint > 0 {
            guard.snapshot.LastUpstreamCheckpoint = event.UpstreamCheckpoint;
        }
        if event.SyncedTS > 0 {
            guard.snapshot.SyncedTS = event.SyncedTS;
        }
        // CheckpointAdvanced carries an authoritative store-progress snapshot. An empty
        // snapshot must clear removed stores, matching Go's non-nil empty map handling.
        if event.SyncedByStoreSet || !event.SyncedByStore.is_empty() {
            guard.snapshot.SyncedByStore = event.SyncedByStore.clone();
        }
        // 仅规划/推进事件更新存活 store 数；失败事件不得抹掉上一轮计数。
        if event.Type == EventType::EventRoundPlanned
            || event.Type == EventType::EventCheckpointAdvanced
        {
            guard.snapshot.AliveStoreCount = event.AliveStoreCount;
        }
        guard.snapshot.PendingFileCount = event.PendingFileCount;
        if let Some(stat) = event.Statistic {
            guard.snapshot.Statistic = new_status_statistic(stat);
        }

        match event.Type {
            EventType::EventCheckpointAdvanced => {
                // 成功推进：恢复 ready，清失败计数与错误文本。
                guard.snapshot.Ready = true;
                guard.snapshot.State = STATE_RUNNING.to_string();
                guard.snapshot.LastSuccessTime = event.Time;
                guard.snapshot.LastError.clear();
                guard.snapshot.LastErrorTime = None;
                guard.snapshot.ConsecutiveFailures = 0;
            }
            EventType::EventCalculationFailed => {
                // 计算失败：降级并累计连续失败次数。
                guard.snapshot.Ready = false;
                guard.snapshot.State = STATE_DEGRADED.to_string();
                if let Some(err) = event.Err {
                    eprintln!("calculation failed: {err}");
                    guard.snapshot.LastError = err.to_string();
                }
                guard.snapshot.LastErrorTime = event.Time;
                guard.snapshot.ConsecutiveFailures += 1;
            }
            _ => {
                // 中间阶段事件：若已 degraded 则保持，否则标为 running。
                if guard.snapshot.State != STATE_DEGRADED {
                    guard.snapshot.State = STATE_RUNNING.to_string();
                }
            }
        }
        observe_status_metrics(&guard.snapshot);
    }

    /// 深拷贝快照（含 map 字段），避免调用方持读锁期间再写。
    pub fn snapshot_copy(&self) -> StatusSnapshot {
        let guard = self.inner.read().expect("status store lock");
        let mut snapshot = guard.snapshot.clone();
        // Clone derive 已拷贝 map，此处再赋一次与 Go 显式 copy map 语义对齐。
        snapshot.SyncedByStore = guard.snapshot.SyncedByStore.clone();
        snapshot.Statistic.PlannedFileSuffixCounts =
            guard.snapshot.Statistic.PlannedFileSuffixCounts.clone();
        snapshot.Statistic.DownstreamCheckFileSuffixCounts = guard
            .snapshot
            .Statistic
            .DownstreamCheckFileSuffixCounts
            .clone();
        snapshot
    }
}

/// Calculator `Observer` 适配器：把事件写入同一 `StatusStore`。
#[derive(Clone)]
pub struct StatusObserver {
    pub(crate) store: StatusStore,
}

impl StatusObserver {
    /// 新一轮计算开始时递增 CurrentRound。
    pub fn BeginCalculationRound(&self) -> u64 {
        self.store.begin_round()
    }

    /// 服务层直接投递事件（如 `record_service_failure`）时走此入口。
    pub fn OnCheckpointEvent(&self, event: CheckpointEvent) {
        self.store.apply_event(event);
    }
}

impl astersql_br_pkg_stream_crr_internal_checkpoint::Observer for StatusObserver {
    /// calculator 回调入口；与上方同名方法共用 `apply_event`。
    fn OnCheckpointEvent(&self, event: CheckpointEvent) {
        self.store.apply_event(event);
    }
}

/// 创建初始为 starting/idle 的状态仓及其 observer；构造时即上报一次 metrics。
/// 返回的 `StatusStore` 与 `StatusObserver` 共享同一 `Arc` 内层状态。
pub(crate) fn new_status_store(task_name: impl Into<String>) -> (StatusStore, StatusObserver) {
    let store = StatusStore {
        inner: Arc::new(RwLock::new(StatusStoreInner {
            snapshot: StatusSnapshot {
                TaskName: task_name.into(),
                State: STATE_STARTING.to_string(),
                Phase: PHASE_IDLE.to_string(),
                ..Default::default()
            },
        })),
    };
    observe_status_metrics(&store.inner.read().expect("status store lock").snapshot);
    let observer = StatusObserver {
        store: store.clone(),
    };
    (store, observer)
}

/// 从 checkpoint 包的 `FileStatistic` 投影到对外 `StatusStatistic`。
/// map 字段 clone，避免与 calculator 内部统计共享可变别名。
fn new_status_statistic(stat: FileStatistic) -> StatusStatistic {
    StatusStatistic {
        UpstreamReadMetaFileCount: stat.UpstreamReadMetaFileCount,
        SkippedStoreSyncedMetaFileCount: stat.SkippedStoreSyncedMetaFileCount,
        EstimatedSyncLogFileCount: stat.EstimatedSyncLogFileCount,
        DownstreamCheckFileCount: stat.DownstreamCheckFileCount,
        PlannedFileSuffixCounts: stat.PlannedFileSuffixCounts.clone(),
        DownstreamCheckFileSuffixCounts: stat.DownstreamCheckFileSuffixCounts.clone(),
    }
}

/// 手写 JSON 编码快照；空 map/空错误字段按 Go omitempty 习惯省略。
/// 返回 `Result` 以保留与 Go 签名对称的错误通道（当前实现不失败）。
pub fn encode_status_snapshot(snapshot: &StatusSnapshot) -> Result<String, Error> {
    let mut json = String::from("{");
    // 字段顺序固定，便于人工 diff 与部分测试做子串匹配。
    append_json_string_field(&mut json, "task_name", &snapshot.TaskName, true);
    append_json_bool_field(&mut json, "live", snapshot.Live, false);
    append_json_bool_field(&mut json, "ready", snapshot.Ready, false);
    append_json_string_field(&mut json, "state", &snapshot.State, false);
    append_json_string_field(&mut json, "phase", &snapshot.Phase, false);
    append_json_number_field(&mut json, "current_round", snapshot.CurrentRound, false);
    append_json_number_field(
        &mut json,
        "last_loop_iteration",
        snapshot.LastLoopIteration,
        false,
    );
    append_json_number_field(
        &mut json,
        "last_upstream_checkpoint",
        snapshot.LastUpstreamCheckpoint,
        false,
    );
    append_json_number_field(&mut json, "safe_checkpoint", snapshot.SafeCheckpoint, false);
    append_json_number_field(&mut json, "synced_ts", snapshot.SyncedTS, false);
    // 空 map 省略，减小噪声并与 Go omitempty 对齐。
    if !snapshot.SyncedByStore.is_empty() {
        append_json_map_u64_field(&mut json, "synced_by_store", &snapshot.SyncedByStore, false);
    }
    append_json_i32_field(
        &mut json,
        "alive_store_count",
        snapshot.AliveStoreCount,
        false,
    );
    append_json_i32_field(
        &mut json,
        "pending_file_count",
        snapshot.PendingFileCount,
        false,
    );
    append_json_statistic_field(&mut json, &snapshot.Statistic, false);
    append_json_time_field(
        &mut json,
        "last_success_time",
        snapshot.LastSuccessTime,
        false,
    )?;
    if !snapshot.LastError.is_empty() {
        append_json_string_field(&mut json, "last_error", &snapshot.LastError, false);
    }
    append_json_time_field(&mut json, "last_error_time", snapshot.LastErrorTime, false)?;
    append_json_number_field(
        &mut json,
        "consecutive_failures",
        snapshot.ConsecutiveFailures,
        false,
    );
    append_json_time_field(&mut json, "last_event_time", snapshot.LastEventTime, false)?;
    // 去掉最后一个多余逗号（若有）再闭合对象。
    if json.ends_with(',') {
        json.pop();
    }
    json.push('}');
    Ok(json)
}

/// 追加字符串字段；`first` 控制是否跳过前导逗号。
/// 值侧经 `escape_json_string`，防止任务名含引号破坏 JSON。
fn append_json_string_field(json: &mut String, key: &str, value: &str, first: bool) {
    if !first {
        json.push(',');
    }
    json.push('"');
    json.push_str(key);
    json.push_str("\":");
    json.push('"');
    escape_json_string(json, value);
    json.push('"');
}

/// 追加布尔字段，输出 JSON `true`/`false` 字面量。
fn append_json_bool_field(json: &mut String, key: &str, value: bool, first: bool) {
    if !first {
        json.push(',');
    }
    json.push('"');
    json.push_str(key);
    json.push_str("\":");
    json.push_str(if value { "true" } else { "false" });
}

/// 追加无符号整数字段；调用方负责把有符号计数安全转成 u64。
fn append_json_number_field(json: &mut String, key: &str, value: u64, first: bool) {
    if !first {
        json.push(',');
    }
    json.push('"');
    json.push_str(key);
    json.push_str("\":");
    json.push_str(&value.to_string());
}

/// 追加有符号计数字段；Go 对应字段为 int，负边界不得转换成超大 u64。
fn append_json_i32_field(json: &mut String, key: &str, value: i32, first: bool) {
    if !first {
        json.push(',');
    }
    json.push('"');
    json.push_str(key);
    json.push_str("\":");
    json.push_str(&value.to_string());
}

/// 时间字段：有值写 Go RFC3339Nano；None 对应 Go time.Time 零值。
fn append_json_time_field(
    json: &mut String,
    key: &str,
    value: Option<SystemTime>,
    first: bool,
) -> Result<(), Error> {
    if !first {
        json.push(',');
    }
    json.push('"');
    json.push_str(key);
    json.push_str("\":");
    match value {
        Some(time) => {
            json.push('"');
            json.push_str(&humantime_rfc3339(time)?);
            json.push('"');
        }
        // Go time.Time zero value is not a pointer and therefore encodes as year 1,
        // rather than JSON null.
        None => json.push_str("\"0001-01-01T00:00:00Z\""),
    }
    Ok(())
}

/// 编码 `map[u64]u64`；键以十进制字符串形式写出，符合 JSON 对象键约束。
fn append_json_map_u64_field(json: &mut String, key: &str, value: &HashMap<u64, u64>, first: bool) {
    if !first {
        json.push(',');
    }
    json.push('"');
    json.push_str(key);
    json.push_str("\":{");
    let mut entries: Vec<_> = value.iter().collect();
    // encoding/json converts integer map keys to text and sorts those strings.
    entries.sort_by_cached_key(|(key, _)| key.to_string());
    let mut first_entry = true;
    for (k, v) in entries {
        if !first_entry {
            json.push(',');
        }
        json.push('"');
        json.push_str(&k.to_string());
        json.push_str("\":");
        json.push_str(&v.to_string());
        first_entry = false;
    }
    json.push('}');
}

/// 嵌套 `statistic` 对象；后缀计数字典为空时省略对应字段。
fn append_json_statistic_field(json: &mut String, stat: &StatusStatistic, first: bool) {
    if !first {
        json.push(',');
    }
    json.push_str("\"statistic\":{");
    append_json_i32_field(
        json,
        "upstream_read_meta_file_count",
        stat.UpstreamReadMetaFileCount,
        true,
    );
    append_json_i32_field(
        json,
        "skipped_store_synced_meta_file_count",
        stat.SkippedStoreSyncedMetaFileCount,
        false,
    );
    append_json_i32_field(
        json,
        "estimated_sync_log_file_count",
        stat.EstimatedSyncLogFileCount,
        false,
    );
    append_json_i32_field(
        json,
        "downstream_check_file_count",
        stat.DownstreamCheckFileCount,
        false,
    );
    if !stat.PlannedFileSuffixCounts.is_empty() {
        append_json_map_string_i32_field(
            json,
            "planned_file_suffix_counts",
            &stat.PlannedFileSuffixCounts,
            false,
        );
    }
    if !stat.DownstreamCheckFileSuffixCounts.is_empty() {
        append_json_map_string_i32_field(
            json,
            "downstream_check_file_suffix_counts",
            &stat.DownstreamCheckFileSuffixCounts,
            false,
        );
    }
    if json.ends_with(',') {
        json.pop();
    }
    json.push('}');
}

/// 编码 `map[string]i32`；键做 JSON 字符串转义，值为十进制整数。
fn append_json_map_string_i32_field(
    json: &mut String,
    key: &str,
    value: &HashMap<String, i32>,
    first: bool,
) {
    if !first {
        json.push(',');
    }
    json.push('"');
    json.push_str(key);
    json.push_str("\":{");
    let mut entries: Vec<_> = value.iter().collect();
    // encoding/json sorts string map keys before writing the JSON object.
    entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    let mut first_entry = true;
    for (k, v) in entries {
        if !first_entry {
            json.push(',');
        }
        json.push('"');
        // 后缀名可能含特殊字符，键侧同样转义。
        escape_json_string(json, k);
        json.push_str("\":");
        json.push_str(&v.to_string());
        first_entry = false;
    }
    json.push('}');
}

/// 转义 JSON 字符串中的引号、反斜杠与常见控制字符。
fn escape_json_string(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000c}' => out.push_str("\\f"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            ch if ch <= '\u{001f}' => write!(out, "\\u{:04x}", ch as u32).expect("write String"),
            ch => out.push(ch),
        }
    }
}

/// `SystemTime` → UTC RFC3339Nano；保留纳秒并正确处理 Unix epoch 之前的时间。
fn humantime_rfc3339(time: SystemTime) -> Result<String, Error> {
    let (secs, nanos) = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => (duration.as_secs() as i64, duration.subsec_nanos()),
        Err(error) => {
            let duration = error.duration();
            if duration.subsec_nanos() == 0 {
                (-(duration.as_secs() as i64), 0)
            } else {
                (
                    -(duration.as_secs() as i64) - 1,
                    1_000_000_000 - duration.subsec_nanos(),
                )
            }
        }
    };
    let mut formatted = time_format_rfc3339(secs)?;
    if nanos > 0 {
        formatted.pop();
        let mut fraction = format!("{nanos:09}");
        while fraction.ends_with('0') {
            fraction.pop();
        }
        formatted.push('.');
        formatted.push_str(&fraction);
        formatted.push('Z');
    }
    Ok(formatted)
}

/// 将 Unix 秒格式化为 `YYYY-MM-DDTHH:MM:SSZ`，不依赖外部时间库。
/// 日界用 86400 秒整除；时分秒由余数分解。
fn time_format_rfc3339(secs: i64) -> Result<String, Error> {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    let (year, month, day) = civil_from_days(days);
    if !(0..10_000).contains(&year) {
        return Err(Error::new(format!(
            "Time.MarshalJSON: year outside of range [0,9999]: {year}"
        )));
    }
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

/// Howard Hinnant 公历算法：从 Unix epoch 起的天数换算年/月/日。
/// 与常见 C++ chrono 实现同源，保证跨平台日期一致。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // 719_468：Unix epoch 相对算法原点的日偏移。
    let z = days + 719_468;
    // 400 年为一 era；负日期向下取整对齐 C++ chrono。
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    // mp<10 时月份为 3..12，否则为 1..2 并进位年份。
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}
