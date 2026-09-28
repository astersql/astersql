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

// Domain metrics 迁移期 Aster 单元测试。
//
// 验证 `InitMetricsVars` 将 Go 侧 label 组合正确绑定到共享 Counter / Gauge，
// 以及重新初始化后句柄被重新绑定、计数值复位。

use std::sync::Mutex;

use astersql_domain_metrics::domain_metrics::{
    GenerateHistoricalStatsFailedCounter, GenerateHistoricalStatsSuccessCounter, InitMetricsVars,
    PlanReplayerCaptureTaskDiscardCounter, PlanReplayerCaptureTaskSendCounter,
    PlanReplayerDumpTaskFailed, PlanReplayerDumpTaskSuccess, PlanReplayerRegisterTaskGauge,
};

/// 串行化本文件测试，避免并发初始化共享静态指标。
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 先初始化 stats 向量，再绑定 Domain 指标变量。
fn initialize() {
    unsafe { astersql_domain_metrics::stats::InitStatsMetrics() };
    InitMetricsVars();
}

/// 读取 Domain Counter 当前值。
fn counter_value(
    counter: &std::sync::LazyLock<std::sync::RwLock<Option<prometheus::Counter>>>,
) -> f64 {
    counter
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .expect("domain counter should be initialized")
        .get()
}

/// 通过 Domain 句柄 inc，断言共享向量上对应 label 的值同步增加。
#[test]
fn init_binds_all_go_label_pairs_to_the_shared_vectors() {
    let _guard = TEST_LOCK.lock().unwrap();
    initialize();

    GenerateHistoricalStatsSuccessCounter
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .inc_by(1.0);
    GenerateHistoricalStatsFailedCounter
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .inc_by(2.0);
    PlanReplayerDumpTaskSuccess
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .inc_by(3.0);
    PlanReplayerDumpTaskFailed
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .inc_by(4.0);
    PlanReplayerCaptureTaskSendCounter
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .inc_by(5.0);
    PlanReplayerCaptureTaskDiscardCounter
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .inc_by(6.0);

    unsafe {
        let historical = astersql_domain_metrics::stats::HistoricalStatsCounter
            .as_ref()
            .unwrap();
        assert_eq!(
            historical.with_label_values(&["generate", "success"]).get(),
            1.0
        );
        assert_eq!(
            historical.with_label_values(&["generate", "fail"]).get(),
            2.0
        );

        let replayer = astersql_domain_metrics::stats::PlanReplayerTaskCounter
            .as_ref()
            .unwrap();
        assert_eq!(replayer.with_label_values(&["dump", "success"]).get(), 3.0);
        assert_eq!(replayer.with_label_values(&["dump", "fail"]).get(), 4.0);
        assert_eq!(replayer.with_label_values(&["capture", "send"]).get(), 5.0);
        assert_eq!(
            replayer.with_label_values(&["capture", "discard"]).get(),
            6.0
        );
    }
}

/// Gauge 与共享句柄指向同一对象；reinit 后旧值清零并重新绑定。
#[test]
fn register_gauge_shares_the_go_package_handle_and_reinit_rebinds() {
    let _guard = TEST_LOCK.lock().unwrap();
    initialize();

    PlanReplayerRegisterTaskGauge
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .set(7.0);
    unsafe {
        assert_eq!(
            astersql_domain_metrics::stats::PlanReplayerRegisterTaskGauge
                .as_ref()
                .unwrap()
                .get(),
            7.0
        );
        // 重新创建 stats 向量，模拟进程内 reinit。
        astersql_domain_metrics::stats::InitStatsMetrics();
    }
    InitMetricsVars();

    assert_eq!(counter_value(&GenerateHistoricalStatsSuccessCounter), 0.0);
    assert_eq!(
        PlanReplayerRegisterTaskGauge
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .get(),
        0.0
    );
}
