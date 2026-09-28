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

// InfoSchema 指标子包的迁移单元测试。
//
// 核对 InitMetricsVars 绑定的 label 与 Go 一致、直方图观测可累计，
// 以及重复初始化不会替换已有 Prometheus 序列句柄。

use astersql_infoschema_metrics::{
    GetLatestCounter, GetTSCounter, GetVersionCounter, HitLatestCounter, HitTSCounter,
    HitVersionCounter, InitMetricsVars, LoadSchemaCounterSnapshot, LoadSchemaDurationLoadAll,
    LoadSchemaDurationLoadDiff, LoadSchemaDurationTotal, init,
};
use prometheus::core::Collector;
use std::collections::BTreeSet;

/// 初始化后各 Counter 的 label 子序列数量与取值路径与 Go 对齐。
#[test]
fn init_binds_the_same_counter_labels_as_go() {
    InitMetricsVars();

    GetLatestCounter.Inc();
    GetTSCounter.Inc();
    GetVersionCounter.Inc();
    HitLatestCounter.Inc();
    HitTSCounter.Inc();
    HitVersionCounter.Inc();
    LoadSchemaCounterSnapshot.Inc();

    // InfoCache：get/hit × latest/ts/version = 6 条子序列。
    let info = astersql_infoschema_metrics::metrics::InfoCacheCounters.collect();
    assert_eq!(info[0].get_metric().len(), 6);
    let info_labels: BTreeSet<_> = info[0]
        .get_metric()
        .iter()
        .map(|metric| {
            metric
                .get_label()
                .iter()
                .map(|label| (label.get_name(), label.get_value()))
                .collect::<BTreeSet<_>>()
        })
        .collect();
    assert_eq!(
        info_labels,
        BTreeSet::from([
            BTreeSet::from([("action", "get"), ("type", "latest")]),
            BTreeSet::from([("action", "get"), ("type", "ts")]),
            BTreeSet::from([("action", "get"), ("type", "version")]),
            BTreeSet::from([("action", "hit"), ("type", "latest")]),
            BTreeSet::from([("action", "hit"), ("type", "ts")]),
            BTreeSet::from([("action", "hit"), ("type", "version")]),
        ])
    );
    // LoadSchemaCounter 仅 snapshot 一条。
    let load = astersql_infoschema_metrics::metrics::LoadSchemaCounter.collect();
    assert_eq!(load[0].get_metric().len(), 1);
    assert_eq!(load[0].get_metric()[0].get_label()[0].get_name(), "type");
    assert_eq!(
        load[0].get_metric()[0].get_label()[0].get_value(),
        "snapshot"
    );
    // 包装句柄与底层 with_label_values 读同一序列。
    assert_eq!(
        GetLatestCounter.Get(),
        astersql_infoschema_metrics::metrics::InfoCacheCounters
            .with_label_values(&["get", "latest"])
            .get()
    );
    assert_eq!(
        HitVersionCounter.Get(),
        astersql_infoschema_metrics::metrics::InfoCacheCounters
            .with_label_values(&["hit", "version"])
            .get()
    );
    assert_eq!(
        LoadSchemaCounterSnapshot.Get(),
        astersql_infoschema_metrics::metrics::LoadSchemaCounter
            .with_label_values(&["snapshot"])
            .get()
    );
}

/// duration 直方图三条 label（total/load-diff/load-all）均可 Observe。
#[test]
fn duration_handles_preserve_go_labels_and_observations() {
    init();
    LoadSchemaDurationTotal.Observe(0.1);
    LoadSchemaDurationLoadDiff.Observe(0.2);
    LoadSchemaDurationLoadAll.Observe(0.3);

    assert_eq!(LoadSchemaDurationTotal.GetSampleCount(), 1);
    assert_eq!(LoadSchemaDurationLoadDiff.GetSampleCount(), 1);
    assert_eq!(LoadSchemaDurationLoadAll.GetSampleCount(), 1);
    let duration = astersql_infoschema_metrics::metrics::LoadSchemaDuration.collect();
    assert_eq!(duration[0].get_metric().len(), 3);
    let duration_labels: BTreeSet<_> = duration[0]
        .get_metric()
        .iter()
        .map(|metric| metric.get_label()[0].get_value())
        .collect();
    assert_eq!(
        duration_labels,
        BTreeSet::from(["load-all", "load-diff", "total"])
    );
}

/// 再次 InitMetricsVars 不得更换已绑定的 Counter 指针。
#[test]
fn repeated_initialization_keeps_existing_metric_series() {
    InitMetricsVars();
    let before = (&*GetTSCounter) as *const prometheus::Counter;
    InitMetricsVars();
    let after = (&*GetTSCounter) as *const prometheus::Counter;
    assert_eq!(before, after);
}
