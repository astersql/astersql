// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.

// 聚合包对外测试入口：转发到 `aggregation_aster_unit_test` 中的共享用例。
//
// 保持与 Go `aggregation_test.go` 同名测试函数，便于对照迁移进度。

use crate::aggregation_aster_unit_test as shared;

/// AVG 加权输入与 DISTINCT 语义。
#[test]
fn TestAvg() {
    shared::avg_and_sum_match_go_weighted_input_and_distinct_cases();
}

/// FinalMode AVG 合并部分 count/sum。
#[test]
fn TestAvgFinalMode() {
    shared::avg_final_mode_combines_partial_count_and_sum();
}

/// SUM 与 AVG 共用加权输入用例。
#[test]
fn TestSum() {
    shared::avg_and_sum_match_go_weighted_input_and_distinct_cases();
}

/// SumInt 下推与帧分类。
#[test]
fn TestCheckAggPushDownSumInt() {
    shared::pushdown_and_frame_classification_matches_go_lists();
}

/// BIT_AND 空集/NULL/Reset。
#[test]
fn TestBitAnd() {
    shared::bit_aggregates_preserve_empty_values_nulls_and_reset();
}

/// BIT_OR 空集/NULL/Reset。
#[test]
fn TestBitOr() {
    shared::bit_aggregates_preserve_empty_values_nulls_and_reset();
}

/// BIT_XOR 空集/NULL/Reset。
#[test]
fn TestBitXor() {
    shared::bit_aggregates_preserve_empty_values_nulls_and_reset();
}

/// COUNT 的 NULL、DISTINCT 与 FinalMode。
#[test]
fn TestCount() {
    shared::count_matches_go_null_distinct_and_final_mode_cases();
}

/// GROUP_CONCAT 分隔符、NULL 与 DISTINCT。
#[test]
fn TestConcat() {
    shared::group_concat_matches_go_separator_null_distinct_and_reset_cases();
}

/// FIRST_ROW 取首行语义。
#[test]
fn TestFirstRow() {
    shared::first_row_max_and_min_match_go_row_order_and_null_cases();
}

/// MAX/MIN 极值与 NULL。
#[test]
fn TestMaxMin() {
    shared::first_row_max_and_min_match_go_row_order_and_null_cases();
}

/// 描述符哈希字段敏感性与 Split。
#[test]
fn TestAggFuncDesc() {
    shared::aggregate_descriptor_hash_tracks_every_go_identity_field();
    shared::split_count_builds_owned_partial_and_final_descriptors();
}
