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

// Ingest 指标初始化与注册的迁移单元测试。
//
// 验证 `InitIngestMetrics` 产出带标签的 Histogram，观察样本可计入对应标签，
// 且 `Register` 后指标名为 `tidb_ingestor_write_ingest_api_duration`。

use crate::{
    IngestAPIDuration, InitIngestMetrics, LabelIngestAPI, LabelWriteAPI, Register,
    WriteAPIDuration, WriteIngestAPIDuration,
};

/// 初始化、打点、注册后检查样本数、指标名与指数桶上界。
#[test]
fn initializes_labeled_histogram_and_registers_it() {
    InitIngestMetrics();

    let histogram = WriteIngestAPIDuration
        .read()
        .unwrap()
        .clone()
        .expect("histogram initialized");
    let write = WriteAPIDuration
        .read()
        .unwrap()
        .clone()
        .expect("write observer initialized");
    let ingest = IngestAPIDuration
        .read()
        .unwrap()
        .clone()
        .expect("ingest observer initialized");

    // 各观察一次，对应标签样本计数应为 1
    write.observe(0.001);
    ingest.observe(1.0);
    assert_eq!(
        histogram
            .with_label_values(&[LabelWriteAPI])
            .get_sample_count(),
        1
    );
    assert_eq!(
        histogram
            .with_label_values(&[LabelIngestAPI])
            .get_sample_count(),
        1
    );

    let registry = prometheus::Registry::new();
    Register(&registry);
    let families = registry.gather();
    assert_eq!(families.len(), 1);
    assert_eq!(
        families[0].name(),
        "tidb_ingestor_write_ingest_api_duration"
    );
    assert_eq!(families[0].help(), "write and ingest API duration");

    // 指数桶：首档 0.001，末档 0.001 * 2^19 = 524.288
    let buckets = families[0].get_metric()[0].get_histogram().get_bucket();
    assert_eq!(buckets.len(), 20);
    assert!((buckets[0].upper_bound() - 0.001).abs() < f64::EPSILON);
    assert!((buckets[19].upper_bound() - 524.288).abs() < 1e-9);
}
