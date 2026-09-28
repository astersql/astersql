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

// 规划器指标句柄与 Go 标签语义对齐的单元测试。
//
// 校验 `type` 标签值（含前导空格）、命中计数器共享真实状态，
// 以及伪估计与耗时直方图可观测。

use prometheus13::core::Collector;

use astersql_planner_core_metrics::planner_core_metrics::*;

/// 从 Collector 暴露的 metric family 中取出指定标签的值。
fn label_value<C: Collector>(collector: &C, label_name: &str) -> String {
    collector
        .collect()
        .into_iter()
        .flat_map(|mut family| family.take_metric().into_iter())
        .flat_map(|mut metric| metric.take_label().into_iter())
        .find(|label| label.get_name() == label_name)
        .map(|label| label.get_value().to_owned())
        .expect("bound metric must expose the requested label")
}

#[test]
/// 命中/未命中选择器标签与 Go 一致，且返回同一底层计数状态。
fn plan_cache_counter_selectors_match_go_labels_and_share_real_state() {
    InitMetricsVars();

    let prepared = GetPlanCacheHitCounter(false);
    let non_prepared = GetPlanCacheHitCounter(true);
    assert_eq!(label_value(&prepared, "type"), "prepared");
    assert_eq!(label_value(&non_prepared, "type"), "non-prepared");

    let before = prepared.get();
    prepared.inc_by(2.0);
    assert_eq!(GetPlanCacheHitCounter(false).get(), before + 2.0);

    assert_eq!(
        label_value(&GetPlanCacheMissCounter(false), "type"),
        "prepared"
    );
    assert_eq!(
        label_value(&GetPlanCacheMissCounter(true), "type"),
        "non-prepared"
    );
    assert_eq!(
        label_value(&GetNonPrepPlanCacheUnsupportedCounter(), "type"),
        "non-prepared-unsupported"
    );
}

#[test]
/// 实例级仪表保留 Go 标签值中的前导空格。
fn instance_metric_selectors_preserve_go_label_values_including_spaces() {
    InitMetricsVars();

    assert_eq!(
        label_value(&GetPlanCacheInstanceNumCounter(false), "type"),
        " session-plan-cache"
    );
    assert_eq!(
        label_value(&GetPlanCacheInstanceNumCounter(true), "type"),
        " instance-plan-cache"
    );
    assert_eq!(
        label_value(&GetPlanCacheInstanceMemoryUsage(false), "type"),
        " session-plan-cache"
    );
    assert_eq!(
        label_value(&GetPlanCacheInstanceMemoryUsage(true), "type"),
        " instance-plan-cache"
    );
    assert_eq!(
        label_value(&GetPlanCacheInstanceEvict(), "type"),
        " instance-plan-cache-last-evict"
    );
}

#[test]
/// 伪估计与查找/克隆耗时句柄可采集样本。
fn pseudo_estimation_and_duration_handles_are_observable() {
    InitMetricsVars();

    assert_eq!(
        label_value(&*PseudoEstimationNotAvailable, "type"),
        "nodata"
    );
    assert_eq!(label_value(&*PseudoEstimationOutdate, "type"), "outdate");
    assert_eq!(
        label_value(&GetPlanCacheLookupDuration(false), "type"),
        " session-plan-cache-lookup"
    );
    assert_eq!(
        label_value(&GetPlanCacheLookupDuration(true), "type"),
        " instance-plan-cache-lookup"
    );
    assert_eq!(
        label_value(&GetPlanCacheCloneDuration(), "type"),
        " instance-plan-cache-clone"
    );

    let observer = GetPlanCacheCloneDuration();
    let before = observer.get_sample_count();
    observer.observe(0.01);
    assert_eq!(observer.get_sample_count(), before + 1);
}
