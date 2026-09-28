// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Package-level summary helpers ported from `br/pkg/summary/summary.go`.
//!
//! 包级薄包装：转发到全局 `LogCollector`，签名与 Go `summary` 包函数一一对应。
//! `LAST_STATUS` 单独记录最近成功标志，供 `Succeed` 在 Summary 之后仍可查询。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::collector::{
    Field, LogCollector, SummaryError, SummaryValue, with_collector, with_collector_result,
};

// 对齐 Go lastStatus：与 collector 内 success 双写，避免仅读 collector 被 Summary 清空影响。
static LAST_STATUS: AtomicBool = AtomicBool::new(false);

/// SetUnit set unit "backup/restore" for summary log.
/// 标记当前任务单位（backup/restore），影响摘要日志文案归类。
pub fn SetUnit(unit: &str) {
    with_collector(|c| c.SetUnit(unit));
}

/// CollectSuccessUnit collects success time costs.
/// 累计成功 unit 的耗时/计数；`arg` 为 Duration 或数值等 SummaryValue。
pub fn CollectSuccessUnit(name: &str, unit_count: i32, arg: SummaryValue) {
    with_collector(|c| c.CollectSuccessUnit(name, unit_count, arg));
}

/// CollectFailureUnit collects fail reason.
/// 按 unit 名记录失败原因；同名重复只保留首次（见 collector 实现）。
pub fn CollectFailureUnit(name: &str, reason: SummaryError) {
    with_collector(|c| c.CollectFailureUnit(name, reason));
}

/// CollectDuration collects log time field.
/// 同名 Duration 在 collector 内相加后再由 Summary 写出。
pub fn CollectDuration(name: &str, t: Duration) {
    with_collector(|c| c.CollectDuration(name, t));
}

/// CollectInt collects log int field.
/// 同名 Int 累加；用于 ranges/kv 等计数类字段。
pub fn CollectInt(name: &str, t: i32) {
    with_collector(|c| c.CollectInt(name, t));
}

/// CollectUint collects log uint64 field.
/// 对应 Go CollectUint → collector.CollectUInt（注意大小写差异）。
pub fn CollectUint(name: &str, t: u64) {
    with_collector(|c| c.CollectUInt(name, t));
}

/// SetSuccessStatus sets final success status.
/// 同时更新 LAST_STATUS 与 collector，保证 Succeed 与摘要模板一致。
pub fn SetSuccessStatus(success: bool) {
    LAST_STATUS.store(success, Ordering::SeqCst);
    with_collector(|c| c.SetSuccessStatus(success));
}

/// Succeed returns whether the last call to `SetSuccessStatus` passes `true`.
/// 读原子标志，不依赖 collector 是否已 Summary 清空内部状态。
pub fn Succeed() -> bool {
    LAST_STATUS.load(Ordering::SeqCst)
}

/// NowDureTime returns the duration between start time and current time
/// 自 collector 起始时刻至今的耗时，用于 total costs 等字段。
pub fn NowDureTime() -> Duration {
    with_collector_result(|c| c.NowDureTime())
}

/// 将起始时间前移 `t`，用于把更早阶段耗时并入 total（对齐 Go 同名函数）。
pub fn AdjustStartTimeToEarlierTime(t: Duration) {
    with_collector(|c| c.AdjustStartTimeToEarlierTime(t));
}

/// Summary outputs summary log.
/// 写出成功/失败摘要并重置内部聚合表，随后可重新 Collect。
pub fn Summary(name: &str) {
    with_collector(|c| c.Summary(name));
}

/// Log outputs log.
/// 透传任意消息与字段到注入的 logFunc，不改变聚合状态。
pub fn Log(msg: &str, fields: &[Field]) {
    with_collector(|c| c.Log(msg, fields));
}
