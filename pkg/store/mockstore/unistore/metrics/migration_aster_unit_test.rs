// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// unistore metrics 从 Go 迁移到 Rust 的行为对齐单测。
//
// 调用 `RegisterMetrics` 后，校验各 `unistore_raft_*` Histogram 的桶数量、
// 首末上界与 Go 侧指数桶参数一致。

use std::collections::HashMap;

use crate::RegisterMetrics;

/// 注册后 gather 全部 unistore_raft_* 指标，并核对桶边界。
#[test]
fn registers_all_unistore_raft_histograms_with_go_buckets() {
    RegisterMetrics();

    // name -> (桶数, 首桶上界, 末桶上界)，与 Go exponential_buckets 对齐。
    let expected: HashMap<&str, (usize, f64, f64)> = HashMap::from([
        (
            "unistore_raft_writer_wait",
            (20, 0.001, 0.001 * 1.5_f64.powi(19)),
        ),
        (
            "unistore_raft_writer_wait_step_1",
            (20, 0.001, 0.001 * 1.5_f64.powi(19)),
        ),
        (
            "unistore_raft_writer_wait_step_2",
            (20, 0.001, 0.001 * 1.5_f64.powi(19)),
        ),
        (
            "unistore_raft_writer_wait_step_3",
            (20, 0.001, 0.001 * 1.5_f64.powi(19)),
        ),
        (
            "unistore_raft_writer_wait_step_4",
            (20, 0.001, 0.001 * 1.5_f64.powi(19)),
        ),
        (
            "unistore_raft_raft_db_update",
            (20, 0.001, 0.001 * 1.5_f64.powi(19)),
        ),
        (
            "unistore_raft_kv_db_update",
            (20, 0.001, 0.001 * 1.5_f64.powi(19)),
        ),
        (
            "unistore_raft_lock_update",
            (15, 0.0001, 0.0001 * 2_f64.powi(14)),
        ),
        (
            "unistore_raft_latch_wait",
            (15, 0.0001, 0.0001 * 2_f64.powi(14)),
        ),
        ("unistore_raft_batch_size", (20, 1.0, 1.5_f64.powi(19))),
    ]);

    let gathered: HashMap<_, _> = prometheus::gather()
        .into_iter()
        .filter(|family| family.name().starts_with("unistore_raft_"))
        .map(|family| (family.name().to_owned(), family))
        .collect();
    assert_eq!(gathered.len(), expected.len());

    for (name, (bucket_count, first, last)) in expected {
        let histogram = gathered[name].get_metric()[0].get_histogram();
        let buckets = histogram.get_bucket();
        assert_eq!(buckets.len(), bucket_count, "{name}");
        assert!((buckets[0].upper_bound() - first).abs() < 1e-12, "{name}");
        assert!(
            (buckets[bucket_count - 1].upper_bound() - last).abs() < 1e-9,
            "{name}"
        );
    }
}
