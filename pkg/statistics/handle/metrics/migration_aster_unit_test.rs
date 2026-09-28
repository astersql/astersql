// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 统计 metrics 迁移期单元测试。
//
// 校验健康度分桶配置与 Go 一致、初始化绑定各标签，以及重复 init 复用全局句柄。

use astersql_statistics_handle_metrics::*;
use std::sync::Mutex;

static METRICS_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 分桶下标、排他上界与标签顺序必须与 Go `HealthyBucketConfigs` 对齐。
#[test]
fn healthy_bucket_configs_match_go_order_bounds_and_labels() {
    let expected = [
        (StatsHealthyBucket0To50, 50, "[0,50)"),
        (StatsHealthyBucket50To55, 55, "[50,55)"),
        (StatsHealthyBucket55To60, 60, "[55,60)"),
        (StatsHealthyBucket60To70, 70, "[60,70)"),
        (StatsHealthyBucket70To80, 80, "[70,80)"),
        (StatsHealthyBucket80To100, 100, "[80,100)"),
        (StatsHealthyBucket100To100, 101, "[100,100]"),
        (StatsHealthyBucketTotal, 0, "[0,100]"),
        (StatsHealthyBucketUnneededAnalyze, 0, "unneeded analyze"),
        (StatsHealthyBucketPseudo, 0, "pseudo"),
    ];

    assert_eq!(HEALTHY_BUCKET_CONFIGS.len(), StatsHealthyBucketCount);
    for (config, expected) in HEALTHY_BUCKET_CONFIGS.iter().zip(expected) {
        assert_eq!((config.index, config.upper_bound, config.label), expected);
    }
}

/// `InitMetricsVars` 应绑定全部 Gauge，并使历史统计 Counter 可递增。
#[test]
fn init_binds_each_gauge_and_historical_counter_labels() {
    let _guard = METRICS_TEST_LOCK
        .lock()
        .expect("metrics test lock must not be poisoned");
    InitMetricsVars();
    assert_eq!(StatsHealthyGauges.len(), StatsHealthyBucketCount);

    for (index, gauge) in StatsHealthyGauges.iter().enumerate() {
        gauge.set(index as f64 + 0.5);
        assert_eq!(gauge.get(), index as f64 + 0.5);
    }

    let success_before = DumpHistoricalStatsSuccessCounter.get();
    let failed_before = DumpHistoricalStatsFailedCounter.get();
    DumpHistoricalStatsSuccessCounter.inc();
    DumpHistoricalStatsFailedCounter.inc_by(2.0);
    assert_eq!(
        DumpHistoricalStatsSuccessCounter.get(),
        success_before + 1.0
    );
    assert_eq!(DumpHistoricalStatsFailedCounter.get(), failed_before + 2.0);
}

/// 重复调用 init 应复用同一组全局指标句柄，已写入的值不丢失。
#[test]
fn repeated_init_reuses_go_global_metric_handles() {
    let _guard = METRICS_TEST_LOCK
        .lock()
        .expect("metrics test lock must not be poisoned");
    InitMetricsVars();
    StatsHealthyGauges[StatsHealthyBucketPseudo].set(17.0);
    InitMetricsVars();
    assert_eq!(StatsHealthyGauges[StatsHealthyBucketPseudo].get(), 17.0);
}
