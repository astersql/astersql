// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 索引使用量收集器集成测试。
//
// 对齐 Go `TestGCIndexUsage` 的 10 表 × 10 索引会话上报、关闭、按索引清理与按表清理场景，
// 并补充样本聚合和访问比例桶边界契约。

use crate::{IndexUsageCollector, IndexUsageSample, new_sample};

/// 聚合查询与行数，并确认 close 后 worker 不再处于运行态。
#[test]
fn canonical_index_usage_aggregates_queries_and_rows_and_tracks_worker_lifecycle() {
    let collector = IndexUsageCollector::default();
    collector.start_worker();
    collector.record(11, 22, new_sample(1, 2, 7, 7));
    collector.record(11, 22, new_sample(2, 3, 5, 10));
    assert_eq!(
        collector.sample(11, 22),
        IndexUsageSample {
            last_used_at: collector.sample(11, 22).last_used_at,
            query_total: 3,
            kv_req_total: 5,
            row_access_total: 12,
            percentage_access: [0, 0, 0, 0, 0, 1, 1],
        }
    );
    collector.close();
    assert!(!collector.is_running());
}

#[test]
fn canonical_gc_index_usage_matches_go_integration_scenario() {
    const TABLE_COUNT: i64 = 10;
    const INDEX_COUNT: i64 = 10;

    let collector = IndexUsageCollector::default();
    collector.start_worker();
    let session = collector.spawn_session();
    for table_id in 0..TABLE_COUNT {
        for index_id in 0..INDEX_COUNT {
            session.update(table_id, index_id, new_sample(1, 2, 3, 4));
        }
    }
    session.flush();
    // Go closes StatsUsage before verification; all flushed samples must remain queryable.
    collector.close();

    let verify = |table_exists: &dyn Fn(i64) -> bool, index_exists: &dyn Fn(i64) -> bool| {
        for table_id in 0..TABLE_COUNT {
            for index_id in 0..INDEX_COUNT {
                let actual = collector.sample(table_id, index_id);
                if table_exists(table_id) && index_exists(index_id) {
                    assert_eq!(actual.query_total, 1);
                    assert_eq!(actual.kv_req_total, 2);
                    assert_eq!(actual.row_access_total, 3);
                    assert_eq!(actual.percentage_access, [0, 0, 0, 0, 0, 1, 0]);
                } else {
                    assert_eq!(actual, IndexUsageSample::default());
                }
            }
        }
    };

    verify(&|_| true, &|_| true);

    // Mirrors dropping every index whose ID is at least 5, then GCIndexUsage.
    collector.gc(|_, index_id| index_id < 5);
    verify(&|_| true, &|index_id| index_id < 5);

    // Mirrors dropping tables at positions 5 through 9, then GCIndexUsage again.
    collector.gc(|table_id, index_id| table_id < 5 && index_id < 5);
    verify(&|table_id| table_id < 5, &|index_id| index_id < 5);
}

#[test]
fn canonical_index_usage_bucket_boundaries_match_go() {
    let cases = [
        (0, 10, [1, 0, 0, 0, 0, 0, 0]),
        (1, 200, [0, 1, 0, 0, 0, 0, 0]),
        (1, 100, [0, 0, 1, 0, 0, 0, 0]),
        (1, 10, [0, 0, 0, 1, 0, 0, 0]),
        (1, 5, [0, 0, 0, 0, 1, 0, 0]),
        (1, 2, [0, 0, 0, 0, 0, 1, 0]),
        (1, 1, [0, 0, 0, 0, 0, 0, 1]),
    ];
    for (rows, total, expected) in cases {
        assert_eq!(new_sample(1, 1, rows, total).percentage_access, expected);
    }
}
