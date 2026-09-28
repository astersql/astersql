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

// Domain 层 Prometheus 指标句柄。
//
// 将共享的 `HistoricalStatsCounter` / `PlanReplayerTaskCounter` /
// `PlanReplayerRegisterTaskGauge` 按 Go 侧 label 组合绑定到本模块静态变量，
// 供 Domain 子系统在业务路径上直接 inc / set。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::{LazyLock, RwLock};

use crate::metrics;

/// 历史统计生成成功次数。
pub static GenerateHistoricalStatsSuccessCounter: LazyLock<RwLock<Option<prometheus::Counter>>> =
    LazyLock::new(|| RwLock::new(None));
/// 历史统计生成失败次数。
pub static GenerateHistoricalStatsFailedCounter: LazyLock<RwLock<Option<prometheus::Counter>>> =
    LazyLock::new(|| RwLock::new(None));

/// Plan Replayer dump 成功次数。
pub static PlanReplayerDumpTaskSuccess: LazyLock<RwLock<Option<prometheus::Counter>>> =
    LazyLock::new(|| RwLock::new(None));
/// Plan Replayer dump 失败次数。
pub static PlanReplayerDumpTaskFailed: LazyLock<RwLock<Option<prometheus::Counter>>> =
    LazyLock::new(|| RwLock::new(None));

/// Plan Replayer capture 任务发送次数。
pub static PlanReplayerCaptureTaskSendCounter: LazyLock<RwLock<Option<prometheus::Counter>>> =
    LazyLock::new(|| RwLock::new(None));
/// Plan Replayer capture 任务丢弃次数。
pub static PlanReplayerCaptureTaskDiscardCounter: LazyLock<RwLock<Option<prometheus::Counter>>> =
    LazyLock::new(|| RwLock::new(None));

/// 当前已注册的 Plan Replayer 任务数（Gauge）。
pub static PlanReplayerRegisterTaskGauge: LazyLock<RwLock<Option<prometheus::Gauge>>> =
    LazyLock::new(|| RwLock::new(None));

// init 对应 Go 的包初始化函数：加载时调用 InitMetricsVars。
/// 包初始化入口：绑定全部 Domain 指标变量。
pub fn init() {
    InitMetricsVars();
}

/// 将 `value` 写入 LazyLock 槽位，覆盖旧句柄。
fn replace<T>(slot: &LazyLock<RwLock<Option<T>>>, value: T) {
    *slot
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value);
}

// InitMetricsVars init domain metrics vars.
/// 从共享 metrics 向量中按 label 取值，填充本模块各静态 Counter / Gauge。
pub fn InitMetricsVars() {
    unsafe {
        // 历史统计：generate × success/fail
        let historical = metrics::HistoricalStatsCounter
            .as_ref()
            .expect("HistoricalStatsCounter should be initialized first");
        replace(
            &GenerateHistoricalStatsSuccessCounter,
            historical.with_label_values(&["generate", "success"]),
        );
        replace(
            &GenerateHistoricalStatsFailedCounter,
            historical.with_label_values(&["generate", "fail"]),
        );

        // Plan Replayer：dump / capture 各结果标签
        let replayer = metrics::PlanReplayerTaskCounter
            .as_ref()
            .expect("PlanReplayerTaskCounter should be initialized first");
        replace(
            &PlanReplayerDumpTaskSuccess,
            replayer.with_label_values(&["dump", "success"]),
        );
        replace(
            &PlanReplayerDumpTaskFailed,
            replayer.with_label_values(&["dump", "fail"]),
        );
        replace(
            &PlanReplayerCaptureTaskSendCounter,
            replayer.with_label_values(&["capture", "send"]),
        );
        replace(
            &PlanReplayerCaptureTaskDiscardCounter,
            replayer.with_label_values(&["capture", "discard"]),
        );

        replace(
            &PlanReplayerRegisterTaskGauge,
            metrics::PlanReplayerRegisterTaskGauge
                .as_ref()
                .expect("PlanReplayerRegisterTaskGauge should be initialized first")
                .clone(),
        );
    }
}
