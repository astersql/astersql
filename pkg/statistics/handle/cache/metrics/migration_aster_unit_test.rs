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

// 统计缓存指标标签绑定的迁移单元测试。
//
// 验证 `InitMetricsVars` 切分出的句柄与 Go 侧标签一致，且与父级向量共享同一 time series。

use astersql_statistics_handle_cache_metrics::{cache_metrics, metrics};

/// 写入各子句柄后，父向量同标签取值应一致；再次 `init()` 不应清零已有计数。
#[test]
fn cache_metric_handles_match_go_labels_and_share_parent_series() {
    metrics::init_parent_metrics();
    cache_metrics::InitMetricsVars();

    // 通过子句柄写入，再从父向量按相同标签读回，确认共享 series。
    unsafe {
        cache_metrics::MissCounter.as_ref().unwrap().inc_by(1.0);
        cache_metrics::HitCounter.as_ref().unwrap().inc_by(2.0);
        cache_metrics::UpdateCounter.as_ref().unwrap().inc_by(3.0);
        cache_metrics::DelCounter.as_ref().unwrap().inc_by(4.0);
        cache_metrics::EvictCounter.as_ref().unwrap().inc_by(5.0);
        cache_metrics::RejectCounter.as_ref().unwrap().inc_by(6.0);
        cache_metrics::CostGauge.as_ref().unwrap().set(7.0);
        cache_metrics::CapacityGauge.as_ref().unwrap().set(8.0);

        let counters = metrics::StatsCacheCounter.as_ref().unwrap();
        assert_eq!(counters.with_label_values(&["miss"]).get(), 1.0);
        assert_eq!(counters.with_label_values(&["hit"]).get(), 2.0);
        assert_eq!(counters.with_label_values(&["update"]).get(), 3.0);
        assert_eq!(counters.with_label_values(&["del"]).get(), 4.0);
        assert_eq!(counters.with_label_values(&["evict"]).get(), 5.0);
        assert_eq!(counters.with_label_values(&["reject"]).get(), 6.0);

        let gauges = metrics::StatsCacheGauge.as_ref().unwrap();
        assert_eq!(gauges.with_label_values(&["track"]).get(), 7.0);
        assert_eq!(gauges.with_label_values(&["capacity"]).get(), 8.0);
    }

    // 再次 init 仅重新绑定句柄，不应重置已写入的指标值。
    cache_metrics::init();
    unsafe {
        assert_eq!(cache_metrics::MissCounter.as_ref().unwrap().get(), 1.0);
        assert_eq!(cache_metrics::CapacityGauge.as_ref().unwrap().get(), 8.0);
    }
}
