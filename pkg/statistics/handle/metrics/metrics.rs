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

// 统计健康度与历史统计相关的 Prometheus 指标定义。
//
// 「健康度」（healthy）衡量统计相对表修改量的新鲜程度，按区间分桶上报 Gauge；
// 历史统计（historical stats）导出成功/失败用 Counter 计数。

#![allow(non_snake_case, non_upper_case_globals)]

use prometheus::{Counter, CounterVec, Gauge, GaugeVec, Opts};
use std::sync::LazyLock;

/// 健康度 [0,50) 分桶下标。
pub const StatsHealthyBucket0To50: usize = 0;
/// 健康度 [50,55) 分桶下标。
pub const StatsHealthyBucket50To55: usize = 1;
/// 健康度 [55,60) 分桶下标。
pub const StatsHealthyBucket55To60: usize = 2;
/// 健康度 [60,70) 分桶下标。
pub const StatsHealthyBucket60To70: usize = 3;
/// 健康度 [70,80) 分桶下标。
pub const StatsHealthyBucket70To80: usize = 4;
/// 健康度 [80,100) 分桶下标。
pub const StatsHealthyBucket80To100: usize = 5;
/// 健康度 [100,100]（满分）分桶下标。
pub const StatsHealthyBucket100To100: usize = 6;
/// 合计桶（兼容旧标签 `[0,100]`）下标。
pub const StatsHealthyBucketTotal: usize = 7;
/// 「无需 ANALYZE」特殊桶下标。
pub const StatsHealthyBucketUnneededAnalyze: usize = 8;
/// 「伪统计」（pseudo，未真实 ANALYZE）特殊桶下标。
pub const StatsHealthyBucketPseudo: usize = 9;
/// 健康度桶总数（含特殊桶）。
pub const StatsHealthyBucketCount: usize = 10;

// HealthyBucketConfig 对应 Go 结构体；UpperBound <= 0 的项目表示特殊类别。
/// 单个健康度桶的配置：下标、排他上界与 Prometheus 标签。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HealthyBucketConfig {
    /// 桶在 `StatsHealthyGauges` 中的下标。
    pub index: usize,
    /// 健康度排他上界；`<= 0` 表示特殊类别（合计 / 无需分析 / 伪统计）。
    pub upper_bound: i64,
    /// 暴露给 Prometheus 的 `type` 标签值。
    pub label: &'static str,
}

// 上界为排他边界；总计、无需分析和 pseudo 桶沿用旧版本标签以保持兼容。
/// 全部健康度桶的静态配置表，顺序与 Go 侧常量下标一致。
pub static HEALTHY_BUCKET_CONFIGS: &[HealthyBucketConfig] = &[
    HealthyBucketConfig {
        index: StatsHealthyBucket0To50,
        upper_bound: 50,
        label: "[0,50)",
    },
    HealthyBucketConfig {
        index: StatsHealthyBucket50To55,
        upper_bound: 55,
        label: "[50,55)",
    },
    HealthyBucketConfig {
        index: StatsHealthyBucket55To60,
        upper_bound: 60,
        label: "[55,60)",
    },
    HealthyBucketConfig {
        index: StatsHealthyBucket60To70,
        upper_bound: 70,
        label: "[60,70)",
    },
    HealthyBucketConfig {
        index: StatsHealthyBucket70To80,
        upper_bound: 80,
        label: "[70,80)",
    },
    HealthyBucketConfig {
        index: StatsHealthyBucket80To100,
        upper_bound: 100,
        label: "[80,100)",
    },
    HealthyBucketConfig {
        index: StatsHealthyBucket100To100,
        upper_bound: 101,
        label: "[100,100]",
    },
    HealthyBucketConfig {
        index: StatsHealthyBucketTotal,
        upper_bound: 0,
        label: "[0,100]",
    },
    HealthyBucketConfig {
        index: StatsHealthyBucketUnneededAnalyze,
        upper_bound: 0,
        label: "unneeded analyze",
    },
    HealthyBucketConfig {
        index: StatsHealthyBucketPseudo,
        upper_bound: 0,
        label: "pseudo",
    },
];

/// 底层 `tidb_statistics_stats_healthy` GaugeVec，按 `type` 标签区分桶。
static StatsHealthyGauge: LazyLock<GaugeVec> = LazyLock::new(|| {
    GaugeVec::new(
        Opts::new("tidb_statistics_stats_healthy", "Gauge of stats healthy"),
        &["type"],
    )
    .expect("stats healthy metric descriptor must be valid")
});

/// 历史统计操作计数器向量，标签为 type / result。
static HistoricalStatsCounter: LazyLock<CounterVec> = LazyLock::new(|| {
    CounterVec::new(
        Opts::new(
            "tidb_statistics_historical_stats",
            "counter of the historical stats operation",
        ),
        &["type", "result"],
    )
    .expect("historical stats metric descriptor must be valid")
});

// Go exposes the child metric handles after package initialization. LazyLock gives the
// same process-wide lifetime while making repeated initialization safe in Rust.
/// 各健康度桶对应的子 Gauge 句柄列表，进程内惰性初始化且可安全重复 force。
pub static StatsHealthyGauges: LazyLock<Vec<Gauge>> = LazyLock::new(|| {
    assert_eq!(
        HEALTHY_BUCKET_CONFIGS.len(),
        StatsHealthyBucketCount,
        "HealthyBucketConfigs length mismatch"
    );
    HEALTHY_BUCKET_CONFIGS
        .iter()
        .map(|cfg| StatsHealthyGauge.with_label_values(&[cfg.label]))
        .collect()
});

/// 历史统计 dump 成功次数计数器。
pub static DumpHistoricalStatsSuccessCounter: LazyLock<Counter> =
    LazyLock::new(|| HistoricalStatsCounter.with_label_values(&["dump", "success"]));
/// 历史统计 dump 失败次数计数器。
pub static DumpHistoricalStatsFailedCounter: LazyLock<Counter> =
    LazyLock::new(|| HistoricalStatsCounter.with_label_values(&["dump", "fail"]));

// Go init 在包加载时注册指标；实现中显式保留同样的初始化入口。
/// 包级初始化入口，对应 Go 的 `init`，强制创建全部指标句柄。
pub fn init() {
    InitMetricsVars();
}

// InitMetricsVars 为每个健康度桶绑定标签，并注册历史统计导出成功/失败计数器。
/// 强制初始化健康度 Gauge 列表与历史统计成功/失败 Counter。
pub fn InitMetricsVars() {
    LazyLock::force(&StatsHealthyGauges);
    LazyLock::force(&DumpHistoricalStatsSuccessCounter);
    LazyLock::force(&DumpHistoricalStatsFailedCounter);
}
