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

// SET_VAR / Hint 可更新系统变量白名单。
//
// `HINT_UPDATABLE_VERIFIED` 列出已验证可通过语句 Hint（SET_VAR）临时改写的变量名；
// `setHintUpdatable` 据此为 `SysVar` 打上 `IsHintUpdatableVerified` 标记。

/// 已验证允许通过 Hint SET_VAR 更新的系统变量名列表（与 Go 侧保持一致）。
pub static HINT_UPDATABLE_VERIFIED: &[&str] = &[
    "tidb_opt_agg_push_down",
    "tidb_opt_derive_topn",
    "tidb_opt_broadcast_cartesian_join",
    "tidb_opt_mpp_outer_join_fixed_build_side",
    "tidb_opt_distinct_agg_push_down",
    "tidb_opt_skew_distinct_agg",
    "tidb_opt_three_stage_distinct_agg",
    "tidb_broadcast_join_threshold_size",
    "tidb_broadcast_join_threshold_count",
    "tidb_prefer_broadcast_join_by_exchange_data_size",
    "tidb_opt_write_row_id",
    "tidb_optimizer_selectivity_level",
    "tidb_enable_new_only_full_group_by_check",
    "tidb_enable_outer_join_reorder",
    "tidb_enable_null_aware_anti_join",
    "tidb_replica_read",
    "tidb_enable_paging",
    "tidb_read_consistency",
    "tidb_distsql_scan_concurrency",
    "tidb_opt_insubq_to_join_and_agg",
    "tidb_opt_prefer_range_scan",
    "tidb_opt_enable_correlation_adjustment",
    "tidb_opt_limit_push_down_threshold",
    "tidb_opt_correlation_threshold",
    "tidb_opt_correlation_exp_factor",
    "tidb_opt_cpu_factor",
    "tidb_opt_copcpu_factor",
    "tidb_opt_tiflash_concurrency_factor",
    "tidb_opt_network_factor",
    "tidb_opt_scan_factor",
    "tidb_opt_desc_factor",
    "tidb_opt_seek_factor",
    "tidb_opt_memory_factor",
    "tidb_opt_disk_factor",
    "tidb_opt_concurrency_factor",
    "tidb_opt_force_inline_cte",
    "tidb_opt_use_invisible_indexes",
    "tidb_opt_index_prune_threshold",
    "tidb_opt_hash_agg_cost_factor",
    "tidb_opt_hash_join_cost_factor",
    "tidb_opt_index_join_cost_factor",
    "tidb_opt_index_lookup_cost_factor",
    "tidb_opt_index_merge_cost_factor",
    "tidb_opt_index_reader_cost_factor",
    "tidb_opt_index_scan_cost_factor",
    "tidb_opt_limit_cost_factor",
    "tidb_opt_merge_join_cost_factor",
    "tidb_opt_sort_cost_factor",
    "tidb_opt_stream_agg_cost_factor",
    "tidb_opt_table_full_scan_cost_factor",
    "tidb_opt_table_range_scan_cost_factor",
    "tidb_opt_table_reader_cost_factor",
    "tidb_opt_table_rowid_scan_cost_factor",
    "tidb_opt_table_tiflash_scan_cost_factor",
    "tidb_opt_topn_cost_factor",
    "tidb_opt_selectivity_factor",
    "tidb_opt_risk_eq_skew_ratio",
    "tidb_opt_risk_range_skew_ratio",
    "tidb_opt_group_ndv_skew_ratio",
    "tidb_opt_scale_ndv_skew_ratio",
    "tidb_opt_always_keep_join_key",
    "tidb_opt_cartesian_join_order_threshold",
    "tidb_index_join_batch_size",
    "tidb_index_lookup_size",
    "tidb_index_serial_scan_concurrency",
    "tidb_init_chunk_size",
    "tidb_allow_batch_cop",
    "tidb_allow_mpp",
    "tidb_enforce_mpp",
    "tidb_max_bytes_before_tiflash_external_join",
    "tidb_max_bytes_before_tiflash_external_group_by",
    "tidb_max_bytes_before_tiflash_external_sort",
    "tidb_max_chunk_size",
    "tidb_min_paging_size",
    "tidb_max_paging_size",
    "tidb_paging_size_bytes",
    "tidb_enable_cascades_planner",
    "tidb_merge_join_concurrency",
    "tidb_index_merge_intersection_concurrency",
    "tidb_opt_projection_push_down",
    "tidb_enable_vectorized_expression",
    "tidb_opt_join_reorder_threshold",
    "tidb_opt_enable_advanced_join_reorder",
    "tidb_enable_index_merge",
    "tidb_enable_no_backslash_escapes_in_like",
    "tidb_enable_extended_stats",
    "tidb_isolation_read_engines",
    "tidb_executor_concurrency",
    "tidb_partition_prune_mode",
    "tidb_enable_index_merge_join",
    "tidb_enable_ordered_result_mode",
    "tidb_enable_pseudo_for_outdated_stats",
    "tidb_stats_load_sync_wait",
    "tidb_cost_model_version",
    "tidb_index_join_double_read_penalty_cost_rate",
    "tidb_default_string_match_selectivity",
    "tidb_enable_prepared_plan_cache",
    "tidb_enable_non_prepared_plan_cache",
    "tidb_plan_cache_max_plan_size",
    "tidb_opt_range_max_size",
    "tidb_opt_advanced_join_hint",
    "tidb_opt_prefix_index_single_scan",
    "tidb_store_batch_size",
    "mpp_version",
    "tidb_enable_inl_join_inner_multi_pattern",
    "tidb_opt_enable_no_decorrelate_in_select",
    "tidb_opt_enable_alternative_logical_plans",
    "tidb_opt_enable_late_materialization",
    "tidb_opt_ordering_index_selectivity_threshold",
    "tidb_opt_ordering_index_selectivity_ratio",
    "tidb_opt_enable_mpp_shared_cte_execution",
    "tidb_opt_fix_control",
    "tidb_runtime_filter_type",
    "tidb_runtime_filter_mode",
    "tidb_session_alias",
    "tidb_opt_objective",
    "mpp_exchange_compression_mode",
    "tidb_allow_fallback_to_tikv",
    "tiflash_fastscan",
    "tiflash_fine_grained_shuffle_batch_size",
    "tiflash_fine_grained_shuffle_stream_count",
    "tidb_hash_join_version",
    "tidb_allow_tiflash_cop",
    "tidb_enable_cache_prepare_stmt",
    "cte_max_recursion_depth",
    "sql_mode",
    "max_execution_time",
    "tidb_max_keys_read",
];

/// 本文件使用的精简系统变量视图：名称与 Hint 可更新标记。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SysVar {
    /// 系统变量名。
    pub Name: String,
    /// 是否已验证可通过 Hint SET_VAR 更新。
    pub IsHintUpdatableVerified: bool,
}

impl SysVar {
    /// 以给定名称构造，默认未标记为 Hint 可更新。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            Name: name.into(),
            IsHintUpdatableVerified: false,
        }
    }
}

/// 遍历变量切片，对落在白名单中的项设置 `IsHintUpdatableVerified`。
pub fn setHintUpdatable(vars: &mut [SysVar]) {
    for var in vars {
        if HINT_UPDATABLE_VERIFIED.contains(&var.Name.as_str()) {
            var.IsHintUpdatableVerified = true;
        }
    }
}
