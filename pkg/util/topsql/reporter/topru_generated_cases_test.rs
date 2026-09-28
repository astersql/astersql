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

// TopRU 生成式用例表驱动测试。
//
// 通过 `CaseSpec` 列表调用 `run_top_ru_case`，覆盖 SQL/Plan 元数据、
// 多记录批处理、RU 阈值以及按用户/SQL/执行计划摘要（digest）聚合等场景。
// RU（Request Unit）是 TiDB 资源计量单位。

#[path = "topru_case_runner_test.rs"]
/// TopRU 用例运行器：按 CaseSpec 驱动聚合与上报断言。
mod topru_case_runner_test;

use topru_case_runner_test::{CaseSpec, run_top_ru_case};

/// 返回表驱动的 TopRU 生成用例规格列表。
fn top_ru_generated_case_specs() -> Vec<CaseSpec> {
    vec![
        CaseSpec {
            goal_id: "sqlmeta_present",
            level: "should",
            description: "payload includes SQLMetas for the triggered marker",
            require_send: false,
            ru_records_min: 0,
            exec_count_min: 0,
            exec_count_sum_min: 0,
            total_ru_min: 0.0,
            sql_meta_match_marker: "topru_gen_sqlmeta",
            plan_meta_required: None,
        },
        CaseSpec {
            goal_id: "planmeta_present",
            level: "should",
            description: "payload includes PlanMetas for the triggered marker",
            require_send: false,
            ru_records_min: 0,
            exec_count_min: 0,
            exec_count_sum_min: 0,
            total_ru_min: 0.0,
            sql_meta_match_marker: "",
            plan_meta_required: Some(true),
        },
        CaseSpec {
            goal_id: "multi_records_batch",
            level: "should",
            description: "payload batches multiple RURecords and preserves counts",
            require_send: false,
            ru_records_min: 2,
            exec_count_min: 0,
            exec_count_sum_min: 2,
            total_ru_min: 0.0,
            sql_meta_match_marker: "",
            plan_meta_required: None,
        },
        CaseSpec {
            goal_id: "total_ru_threshold",
            level: "should",
            description: "payload contains a record with RU above a threshold",
            require_send: false,
            ru_records_min: 0,
            exec_count_min: 0,
            exec_count_sum_min: 0,
            total_ru_min: 1.5,
            sql_meta_match_marker: "",
            plan_meta_required: None,
        },
        CaseSpec {
            goal_id: "key_aggregation_by_user_sql_plan",
            level: "must",
            description: "aggregation key is (user, sql_digest, plan_digest); different users same SQL are separated",
            require_send: true,
            ru_records_min: 0,
            exec_count_min: 0,
            exec_count_sum_min: 0,
            total_ru_min: 0.0,
            sql_meta_match_marker: "",
            plan_meta_required: None,
        },
        CaseSpec {
            goal_id: "same_timestamp_multiple_finish_accumulate",
            level: "should",
            description: "within same timestamp, multiple finishes for same key accumulate ruIncrement into TopN",
            require_send: true,
            ru_records_min: 1,
            exec_count_min: 0,
            exec_count_sum_min: 2,
            total_ru_min: 0.0,
            sql_meta_match_marker: "",
            plan_meta_required: None,
        },
        CaseSpec {
            goal_id: "internal_sql_empty_user_handling",
            level: "should",
            description: "empty user (internal SQL) is handled deterministically (no panic, stable key)",
            require_send: true,
            ru_records_min: 0,
            exec_count_min: 0,
            exec_count_sum_min: 0,
            total_ru_min: 0.0,
            sql_meta_match_marker: "",
            plan_meta_required: None,
        },
        CaseSpec {
            goal_id: "short_exec_time_lt_1s_handling",
            level: "should",
            description: "exec_duration < 1s still produces correct RU record (or explicitly skipped by design)",
            require_send: true,
            ru_records_min: 0,
            exec_count_min: 0,
            exec_count_sum_min: 0,
            total_ru_min: 0.0,
            sql_meta_match_marker: "",
            plan_meta_required: None,
        },
    ]
}

// Go: TestTopRUGeneratedCases and its eight table-driven subtests.
#[test]
#[serial_test::serial]
/// 对应 Go `TestTopRUGeneratedCases`：逐条执行生成用例。
fn test_top_ru_generated_cases() {
    for case in top_ru_generated_case_specs() {
        run_top_ru_case(&case);
    }
}
