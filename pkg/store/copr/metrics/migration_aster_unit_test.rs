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

// Copr 缓存指标迁移对齐的单元测试。
//
// 验证 `LazyLock` 拆分出的 evict/hit/miss 计数器与底层 `CounterVec`
// 各 label 共享同一累加状态，确保迁移后指标语义与 Go 一致。

use crate::copr_metrics::{CoprCacheCounterEvict, CoprCacheCounterHit, CoprCacheCounterMiss, init};

/// 初始化后分别递增三个 label，并断言独立计数器与 CounterVec 读数一致。
#[test]
fn migration_initializes_distinct_copr_cache_label_counters() {
    // 先注册 DistSQL CounterVec，再 force 惰性全局计数器。
    crate::metrics::init_dist_sql_metrics();
    init();

    let evict_before = CoprCacheCounterEvict.get();
    let hit_before = CoprCacheCounterHit.get();
    let miss_before = CoprCacheCounterMiss.get();

    CoprCacheCounterEvict.inc_by(1.0);
    CoprCacheCounterHit.inc_by(2.0);
    CoprCacheCounterMiss.inc_by(3.0);

    assert_eq!(CoprCacheCounterEvict.get(), evict_before + 1.0);
    assert_eq!(CoprCacheCounterHit.get(), hit_before + 2.0);
    assert_eq!(CoprCacheCounterMiss.get(), miss_before + 3.0);

    // 通过底层 CounterVec 再读一遍，确认 label 与独立 Counter 同源。
    let cache_counter = unsafe {
        crate::metrics::DistSQLCoprCacheCounter
            .as_ref()
            .expect("DistSQL coprocessor cache counter must be initialized")
    };
    assert_eq!(
        cache_counter.with_label_values(&["evict"]).get(),
        evict_before + 1.0
    );
    assert_eq!(
        cache_counter.with_label_values(&["hit"]).get(),
        hit_before + 2.0
    );
    assert_eq!(
        cache_counter.with_label_values(&["miss"]).get(),
        miss_before + 3.0
    );
}
