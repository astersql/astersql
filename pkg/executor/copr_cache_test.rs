// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Coprocessor（协处理器）缓存命中率统计的单元测试。
//
// Coprocessor 将部分计算下推到存储层（如 TiKV）执行。缓存命中率
// `calcCacheHit` 的分母需计入普通响应次数与 store batch（多请求合并）次数，
// 避免把批处理请求算作单次响应而抬高命中率。

use std::time::Duration;

use astersql_config::{get_global_config, store_global_config};
use astersql_distsql::select_result::selectResultRuntimeStats;

struct GlobalConfigGuard(astersql_config::Config);

impl Drop for GlobalConfigGuard {
    fn drop(&mut self) {
        store_global_config(self.0.clone());
    }
}

/// 验证缓存命中率分母包含 store batch 次数，且 fallback 计数正确。
///
/// 初始 `response_count=3`、`cop_cache_hit_num=1`、`store_batched_num=1`，
/// 再 merge 一次带缓存命中与 batch 的运行时统计后，命中率应为
/// (1+1) / (3+1+1) = 0.4。
#[test]
fn coprocessor_cache_ratio_counts_store_batches_in_denominator() {
    let original = get_global_config();
    let _config_guard = GlobalConfigGuard(original.as_ref().clone());
    let mut enabled = original.as_ref().clone();
    enabled.tikv_client.copr_cache.capacity_mb = 1000;
    store_global_config(enabled);

    let mut stats = selectResultRuntimeStats {
        response_count: 3,
        cop_cache_hit_num: 1,
        store_batched_num: 1,
        ..Default::default()
    };
    // 合并一次 Cop 运行时统计：耗时 5ms，命中缓存，并产生 1 次 store batch
    stats.mergeCopRuntimeStats(Duration::from_millis(5), true, 1, 1);
    assert_eq!(stats.calcCacheHit(), 0.4);
    assert_eq!(stats.store_batched_fallback_num, 1);
    assert!(stats.to_string().contains("cache_hit_ratio: 0.40"));

    let mut cold_stats = selectResultRuntimeStats {
        response_count: 5,
        ..Default::default()
    };
    cold_stats.mergeCopRuntimeStats(Duration::from_millis(5), false, 0, 0);
    assert_eq!(cold_stats.calcCacheHit(), 0.0);
    assert!(cold_stats.to_string().contains("cache_hit_ratio: 0.00"));

    let mut disabled = get_global_config().as_ref().clone();
    disabled.tikv_client.copr_cache.capacity_mb = 0;
    store_global_config(disabled);
    assert!(cold_stats.to_string().contains("copr_cache: disabled"));
    assert!(!cold_stats.to_string().contains("cache_hit_ratio"));
}
