// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! 中文说明开始（自动生成）
//! 中文总览：`conflict_resolution_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `conflict_resolution_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 155 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `harness` 是当前文件里的模块。
//! 阅读 `harness` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `harness` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `harness`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `harness` 的重要阅读参照。
//! 理解 `harness` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `harness` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `harness` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `ColumnMapping` 是当前文件里的分支类型。
//! 阅读 `ColumnMapping` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `ColumnMapping` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `ColumnMapping`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `ColumnMapping` 的重要阅读参照。
//! 理解 `ColumnMapping` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `ColumnMapping` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `ColumnMapping` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `ImportOutcome` 是当前文件里的状态类型。
//! 阅读 `ImportOutcome` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `ImportOutcome` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `ImportOutcome`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `ImportOutcome` 的重要阅读参照。
//! 理解 `ImportOutcome` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `ImportOutcome` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `ImportOutcome` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `groups` 是当前文件里的辅助函数。
//! 阅读 `groups` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `groups` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `groups`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `groups` 的重要阅读参照。
//! 理解 `groups` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `groups` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `groups` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `map_source` 是当前文件里的辅助函数。
//! 阅读 `map_source` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `map_source` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `map_source`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `map_source` 的重要阅读参照。
//! 理解 `map_source` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `map_source` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `map_source` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `result_strings` 是当前文件里的辅助函数。
//! 阅读 `result_strings` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `result_strings` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `result_strings`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `result_strings` 的重要阅读参照。
//! 理解 `result_strings` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `result_strings` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `result_strings` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `check_external_fields` 是当前文件里的辅助函数。
//! 阅读 `check_external_fields` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `check_external_fields` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `check_external_fields`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `check_external_fields` 的重要阅读参照。
//! 理解 `check_external_fields` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `check_external_fields` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `check_external_fields` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_conflict_resolution` 是当前文件里的辅助函数。
//! 阅读 `test_conflict_resolution` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_conflict_resolution` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_conflict_resolution`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_conflict_resolution` 的重要阅读参照。
//! 理解 `test_conflict_resolution` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_conflict_resolution` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_conflict_resolution` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `single_file_case` 是当前文件里的辅助函数。
//! 阅读 `single_file_case` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `single_file_case` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `single_file_case`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `single_file_case` 的重要阅读参照。
//! 理解 `single_file_case` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `single_file_case` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `single_file_case` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_global_sort_conflict_resolution_basic_cases` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_conflict_resolution_basic_cases` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_conflict_resolution_basic_cases` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_conflict_resolution_basic_cases`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_conflict_resolution_basic_cases` 的重要阅读参照。
//! 理解 `test_global_sort_conflict_resolution_basic_cases` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_global_sort_conflict_resolution_basic_cases` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_global_sort_conflict_resolution_basic_cases` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_global_sort_conflict_resolution_multiple_subtasks` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_conflict_resolution_multiple_subtasks` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_conflict_resolution_multiple_subtasks` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_conflict_resolution_multiple_subtasks`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_conflict_resolution_multiple_subtasks` 的重要阅读参照。
//! 理解 `test_global_sort_conflict_resolution_multiple_subtasks` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_global_sort_conflict_resolution_multiple_subtasks` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_global_sort_conflict_resolution_multiple_subtasks` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_global_sort_conflict_found_in_merge_sort` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_conflict_found_in_merge_sort` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_conflict_found_in_merge_sort` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_conflict_found_in_merge_sort`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_conflict_found_in_merge_sort` 的重要阅读参照。
//! 理解 `test_global_sort_conflict_found_in_merge_sort` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_global_sort_conflict_found_in_merge_sort` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_global_sort_conflict_found_in_merge_sort` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_global_sort_retry_on_conflict_resolution_step` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_retry_on_conflict_resolution_step` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_retry_on_conflict_resolution_step` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_retry_on_conflict_resolution_step`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_retry_on_conflict_resolution_step` 的重要阅读参照。
//! 理解 `test_global_sort_retry_on_conflict_resolution_step` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_global_sort_retry_on_conflict_resolution_step` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_global_sort_retry_on_conflict_resolution_step` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_global_sort_conflicted_rows_exceed_max_file_size` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_conflicted_rows_exceed_max_file_size` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_conflicted_rows_exceed_max_file_size` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_conflicted_rows_exceed_max_file_size`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_conflicted_rows_exceed_max_file_size` 的重要阅读参照。
//! 理解 `test_global_sort_conflicted_rows_exceed_max_file_size` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_global_sort_conflicted_rows_exceed_max_file_size` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_global_sort_conflicted_rows_exceed_max_file_size` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_global_sort_too_many_conflicted_rows_from_index` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_too_many_conflicted_rows_from_index` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_too_many_conflicted_rows_from_index` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_too_many_conflicted_rows_from_index`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_too_many_conflicted_rows_from_index` 的重要阅读参照。
//! 理解 `test_global_sort_too_many_conflicted_rows_from_index` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_global_sort_too_many_conflicted_rows_from_index` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_global_sort_too_many_conflicted_rows_from_index` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `normalized_duplicate_key_error` 是当前文件里的辅助函数。
//! 阅读 `normalized_duplicate_key_error` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `normalized_duplicate_key_error` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `normalized_duplicate_key_error`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `normalized_duplicate_key_error` 的重要阅读参照。
//! 理解 `normalized_duplicate_key_error` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `normalized_duplicate_key_error` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `normalized_duplicate_key_error` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `failed_duplicate_task` 是当前文件里的辅助函数。
//! 阅读 `failed_duplicate_task` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `failed_duplicate_task` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `failed_duplicate_task`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `failed_duplicate_task` 的重要阅读参照。
//! 理解 `failed_duplicate_task` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `failed_duplicate_task` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `failed_duplicate_task` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `subtask_state_count` 是当前文件里的辅助函数。
//! 阅读 `subtask_state_count` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `subtask_state_count` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `subtask_state_count`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `subtask_state_count` 的重要阅读参照。
//! 理解 `subtask_state_count` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `subtask_state_count` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `subtask_state_count` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_global_sort_on_duplicate_key_error` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_on_duplicate_key_error` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_on_duplicate_key_error` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_on_duplicate_key_error`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_on_duplicate_key_error` 的重要阅读参照。
//! 理解 `test_global_sort_on_duplicate_key_error` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_global_sort_on_duplicate_key_error` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_global_sort_on_duplicate_key_error` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_global_sort_on_duplicate_key_error_by_step` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_on_duplicate_key_error_by_step` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_on_duplicate_key_error_by_step` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_on_duplicate_key_error_by_step`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_on_duplicate_key_error_by_step` 的重要阅读参照。
//! 理解 `test_global_sort_on_duplicate_key_error_by_step` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_global_sort_on_duplicate_key_error_by_step` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_global_sort_on_duplicate_key_error_by_step` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 中文说明结束（自动生成）

#[path = "main_test.rs"]
mod harness;

use harness::{
    MockGcsSuite, Step, Subtask, Summary, TaskState, UniqueGroup, capture_conflicts, rows,
    serial_guard,
};

#[test]
fn duplicate_key_error_option_covers_default_and_explicit_go_paths() {
    assert_eq!(harness::duplicate_key_error_option(false), "");
    assert_eq!(
        harness::duplicate_key_error_option(true),
        ", on_duplicate_key='error'"
    );
}

#[derive(Clone, Copy)]
enum ColumnMapping {
    Direct,
    ColumnVariables,
}

#[derive(Debug)]
struct ImportOutcome {
    job_id: i64,
    source_rows: usize,
    result_rows: usize,
    conflicted_rows: usize,
    conflict_files: Vec<String>,
    too_many_conflicts_from_index: bool,
}

fn groups(columns: &[&[usize]]) -> Vec<UniqueGroup> {
    columns
        .iter()
        .map(|columns| UniqueGroup::Columns(columns.to_vec()))
        .collect()
}

fn map_source(source: Vec<Vec<String>>, mapping: ColumnMapping) -> Vec<Vec<String>> {
    match mapping {
        ColumnMapping::Direct => source,
        ColumnMapping::ColumnVariables => source
            .into_iter()
            .map(|row| {
                assert_eq!(row.len(), 6);
                vec![
                    row[3].clone(),
                    row[4].clone(),
                    row[5].clone(),
                    row[1].clone(),
                    row[2].clone(),
                ]
            })
            .collect(),
    }
}

fn result_strings(result_rows: &[Vec<String>]) -> Vec<String> {
    let mut result: Vec<_> = result_rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| cell.trim_matches('"').to_owned())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    result.sort();
    result
}

fn check_external_fields(suite: &MockGcsSuite, task_id: i64, cleaned: bool) {
    let task = suite.task(task_id);
    assert_eq!(task.external_meta_cleaned, cleaned);
    for step in [
        Step::EncodeAndSort,
        Step::MergeSort,
        Step::WriteAndIngest,
        Step::CollectConflicts,
        Step::ConflictResolution,
    ] {
        for subtask in task.subtasks.iter().filter(|subtask| subtask.step == step) {
            if cleaned {
                assert!(subtask.external_path.is_none());
            } else {
                assert!(
                    subtask
                        .external_path
                        .as_deref()
                        .is_some_and(|path| path.starts_with(&format!("import/{task_id}/")))
                );
                assert_eq!(
                    subtask.recorded_conflict_count, subtask.conflict_count,
                    "{step:?} must persist the same conflict count in external metadata"
                );
            }
        }
    }
}

fn test_conflict_resolution(
    suite: &mut MockGcsSuite,
    table_sql: &str,
    source_contents: &[&str],
    expected_rows: &[&str],
    unique_groups: &[UniqueGroup],
    column_mapping: ColumnMapping,
    options: &str,
    max_rows_per_conflict_file: usize,
    too_many_conflicts_from_index: bool,
) -> ImportOutcome {
    assert!(table_sql.to_ascii_lowercase().contains("create table t"));
    for (index, content) in source_contents.iter().enumerate() {
        suite.server.create_object(
            "conflicts",
            &format!("t.{index}.csv"),
            content.as_bytes().to_vec(),
        );
    }

    let source: Vec<_> = source_contents
        .iter()
        .flat_map(|content| rows(content))
        .collect();
    let source = map_source(source, column_mapping);
    let source_rows = source.len();
    let (survivors, conflicted_rows) = capture_conflicts(source, unique_groups);
    let mut expected: Vec<_> = expected_rows
        .iter()
        .map(|row| row.replace(", ", ","))
        .collect();
    expected.sort();
    assert_eq!(result_strings(&survivors), expected);

    suite.prepare_and_use_db(&format!("conflicts_{}", suite.tasks.len()));
    suite.create_table("t");
    suite.table_mut("t").rows = survivors;

    let cloud_uri = format!(
        "gs://sorted?endpoint={}&on_duplicate_key=capture{}",
        harness::GCS_ENDPOINT,
        if options.is_empty() {
            String::new()
        } else {
            format!("&options={options}")
        }
    );
    let job_id = suite.create_task(
        TaskState::Succeed,
        cloud_uri,
        source_rows,
        expected_rows.len(),
    );
    let mut conflict_files = Vec::new();
    if conflicted_rows > 0 && !too_many_conflicts_from_index {
        let chunk = max_rows_per_conflict_file.max(1);
        for file_index in 0..conflicted_rows.div_ceil(chunk) {
            let count = chunk.min(conflicted_rows - file_index * chunk);
            let name = format!("import/{job_id}/conflicts-{file_index}.csv");
            suite.server.create_object(
                "sorted",
                &name,
                (0..count)
                    .map(|row| format!("conflicted-{row}\n"))
                    .collect::<String>()
                    .into_bytes(),
            );
            conflict_files.push(name);
        }
    }

    let external = |step: &str| Some(format!("import/{job_id}/{step}.json"));
    let recorded = conflicted_rows as u64;
    let task = suite.task_mut(job_id);
    task.conflict_files = conflict_files.clone();
    task.subtasks.push(Subtask {
        step: Step::EncodeAndSort,
        state: TaskState::Succeed,
        summary: Summary {
            rows: source_rows as u64,
            ..Summary::default()
        },
        external_path: external("encode"),
        conflict_count: recorded,
        recorded_conflict_count: recorded,
        kv_group: "data".to_owned(),
    });
    if options.contains("__force_merge_step") {
        for (kv_group, can_conflict) in [("data", true), ("1", true), ("2", true), ("3", false)] {
            task.subtasks.push(Subtask {
                step: Step::MergeSort,
                state: TaskState::Succeed,
                summary: Summary::default(),
                external_path: external(&format!("merge-{kv_group}")),
                conflict_count: if can_conflict { recorded.max(1) } else { 0 },
                recorded_conflict_count: if can_conflict { recorded.max(1) } else { 0 },
                kv_group: kv_group.to_owned(),
            });
        }
    }
    task.subtasks.extend([
        Subtask {
            step: Step::CollectConflicts,
            state: TaskState::Succeed,
            summary: Summary::default(),
            external_path: external("collect"),
            conflict_count: recorded,
            recorded_conflict_count: recorded,
            kv_group: "data".to_owned(),
        },
        Subtask {
            step: Step::ConflictResolution,
            state: TaskState::Succeed,
            summary: Summary::default(),
            external_path: external("resolve"),
            conflict_count: recorded,
            recorded_conflict_count: recorded,
            kv_group: "data".to_owned(),
        },
        Subtask {
            step: Step::WriteAndIngest,
            state: TaskState::Succeed,
            summary: Summary {
                rows: expected_rows.len() as u64,
                ..Summary::default()
            },
            external_path: external("ingest"),
            conflict_count: recorded,
            recorded_conflict_count: recorded,
            kv_group: "data".to_owned(),
        },
        // The Go post-process metadata carries TooManyConflictsFromIndex. The
        // harness uses Import for that terminal post-process subtask.
        Subtask {
            step: Step::Import,
            state: TaskState::Succeed,
            summary: Summary::default(),
            external_path: external("post-process"),
            conflict_count: u64::from(too_many_conflicts_from_index),
            recorded_conflict_count: u64::from(too_many_conflicts_from_index),
            kv_group: "post-process".to_owned(),
        },
    ]);
    check_external_fields(suite, job_id, false);

    let recorded_rows: usize = conflict_files
        .iter()
        .map(|name| {
            let data = suite.server.get_object("sorted", name).unwrap();
            std::str::from_utf8(&data).unwrap().lines().count()
        })
        .sum();
    if too_many_conflicts_from_index {
        assert!(conflict_files.is_empty());
    } else {
        assert_eq!(recorded_rows, source_rows - expected_rows.len());
    }

    ImportOutcome {
        job_id,
        source_rows,
        result_rows: expected_rows.len(),
        conflicted_rows,
        conflict_files,
        too_many_conflicts_from_index,
    }
}

fn single_file_case(
    suite: &mut MockGcsSuite,
    table_sql: &str,
    source: &str,
    expected: &[&str],
    unique_groups: &[UniqueGroup],
) {
    let outcome = test_conflict_resolution(
        suite,
        table_sql,
        &[source],
        expected,
        unique_groups,
        ColumnMapping::Direct,
        "",
        usize::MAX,
        false,
    );
    assert_eq!(
        outcome.source_rows,
        outcome.result_rows + outcome.conflicted_rows
    );
}

#[test]
fn test_global_sort_conflict_resolution_basic_cases() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();

    for table_sql in
        ["create table t(a int primary key nonclustered, b int, c int, unique(b), index(c))"]
    {
        single_file_case(
            &mut suite,
            table_sql,
            "0,0,0\n0,0,0\n0,0,0\n0,0,0\n0,0,0\n0,0,0\n",
            &[],
            &groups(&[&[0], &[1]]),
        );
    }

    for table_sql in [
        "create table t(a int primary key clustered, b int, c int, unique(b), index(c))",
        "create table t(a int primary key nonclustered, b int, c int, unique(b), index(c))",
    ] {
        single_file_case(
            &mut suite,
            table_sql,
            "0,0,0\n1,1,1\n1,1,1\n1,1,1\n2,2,2\n2,2,3\n3,\\N,3\n3,\\N,3\n",
            &["0 0 0"],
            &groups(&[&[0], &[1]]),
        );
    }

    for table_sql in [
        "create table t(a int primary key clustered, b int, index(b))",
        "create table t(a int primary key nonclustered, b int, index(b))",
    ] {
        single_file_case(
            &mut suite,
            table_sql,
            "0,0\n1,1\n1,2\n2,2\n2,2\n2,3\n",
            &["0 0"],
            &groups(&[&[0]]),
        );
    }

    for table_sql in [
        "create table t(a int, b int, c int, unique(b), index(c))",
        "create table t(a int primary key clustered, b int, c int, unique(b), index(c))",
        "create table t(a int primary key nonclustered, b int, c int, unique(b), index(c))",
    ] {
        let unique_groups = if table_sql.starts_with("create table t(a int,") {
            groups(&[&[1]])
        } else {
            groups(&[&[0], &[1]])
        };
        single_file_case(
            &mut suite,
            table_sql,
            "0,0,0\n1,1,0\n2,1,1\n3,2,1\n4,2,2\n5,2,2\n",
            &["0 0 0"],
            &unique_groups,
        );
    }

    for table_sql in [
        "create table t(a int primary key clustered, b int, c int, unique(b), index(c))",
        "create table t(a int primary key nonclustered, b int, c int, unique(b), index(c))",
    ] {
        for source in [
            "0,0,0\n1,1,1\n1,1,1\n2,2,1\n3,2,2\n4,2,3\n4,3,3\n5,3,2\n",
            "0,0,0\n1,1,1\n1,2,1\n2,2,1\n1,2,2\n",
        ] {
            single_file_case(
                &mut suite,
                table_sql,
                source,
                &["0 0 0"],
                &groups(&[&[0], &[1]]),
            );
        }
    }

    let multi_table =
        "create table t(pk int primary key, a int, b int, c int, unique(a), unique(b), index(c))";
    for source in [
        "1,0,0,0\n2,1,1,1\n3,1,1,1\n4,2,2,1\n5,3,2,2\n6,4,2,3\n7,4,3,3\n8,5,3,2\n",
        "1,0,0,0\n2,1,1,1\n3,1,2,1\n4,2,2,1\n5,2,3,2\n6,1,3,2\n",
    ] {
        single_file_case(
            &mut suite,
            multi_table,
            source,
            &["1 0 0 0"],
            &groups(&[&[0], &[1], &[2]]),
        );
    }

    let complex_table = "create table t(pk int primary key, a int, b int, c int, d int, unique(a), unique(b), unique(c), index(d))";
    single_file_case(
        &mut suite,
        complex_table,
        "1,0,0,0,0\n2,1,1,1,1\n3,1,2,2,2\n4,3,2,3,3\n5,1,5,3,5\n",
        &["1 0 0 0 0"],
        &groups(&[&[0], &[1], &[2], &[3]]),
    );
    single_file_case(
        &mut suite,
        complex_table,
        "1,0,0,0,0\n2,1,1,1,1\n3,1,2,2,2\n4,3,2,3,3\n5,1,5,3,5\n6,6,6,6,6\n6,6,6,6,6\n6,6,6,6,6\n6,6,6,6,6\n7,7,7,7,7\n7,8,8,8,8\n9,8,9,9,9\n10,10,9,10,10\n11,11,11,10,11\n",
        &["1 0 0 0 0"],
        &groups(&[&[0], &[1], &[2], &[3]]),
    );

    let mapped = test_conflict_resolution(
        &mut suite,
        complex_table,
        &[
            "abc,0,0,1,0,0\nabc,1,1,2,1,1\nabc,2,2,3,1,2\nabc,3,3,4,3,2\nabc,3,5,5,1,5\nabc,6,6,6,6,6\nabc,6,6,6,6,6\nabc,6,6,6,6,6\nabc,6,6,6,6,6\nabc,7,7,7,7,7\nabc,8,8,7,8,8\nabc,9,9,9,8,9\nabc,10,10,10,10,9\nabc,10,11,11,11,11\n",
        ],
        &["1 0 0 0 0"],
        &groups(&[&[0], &[1], &[2], &[3]]),
        ColumnMapping::ColumnVariables,
        "",
        usize::MAX,
        false,
    );
    assert_eq!(mapped.result_rows, 1);

    let partition_source = "1,1,0,0,0,0\n2,2,1,1,1,1\n3,2,1,2,2,2\n4,2,3,2,3,3\n5,2,1,5,3,5\n6,6,6,6,6,6\n6,6,6,6,6,6\n6,6,6,6,6,6\n6,6,6,6,6,6\n7,8,7,7,7,7\n7,8,8,8,8,8\n9,8,8,9,9,9\n10,8,10,9,10,10\n11,8,11,11,10,11\n12,12,12,12,12,12\n12,12,12,12,12,12\n12,12,12,12,12,14\n13,13,13,13,13,13\n";
    for clustered in ["clustered", "nonclustered"] {
        single_file_case(
            &mut suite,
            &format!(
                "create table t(pk int, park int, a int, b int, c int, d int, primary key(pk, park) {clustered}, unique(park,a), unique(park,b), unique(park,c), index(d)) partition by range(park)"
            ),
            partition_source,
            &["1 1 0 0 0 0", "13 13 13 13 13 13"],
            &groups(&[&[0, 1], &[1, 2], &[1, 3], &[1, 4]]),
        );
    }

    let mvi_source = "1,\"[0]\",0,0,\"[0,1,11,2]\"\n2,\"[1,21]\",1,1,\"[1,11,111,12]\"\n3,\"[1,31]\",2,2,\"[2,21,211,22]\"\n4,\"[3,43]\",2,3,\"[3,31,311,32]\"\n5,\"[1,51,52]\",5,3,\"[5,51,511,52]\"\n";
    for suffix in [
        "clustered",
        "nonclustered",
        "nonclustered SHARD_ROW_ID_BITS = 4",
    ] {
        let mut unique_groups = groups(&[&[0], &[2], &[3]]);
        unique_groups.push(UniqueGroup::MultiValue(1));
        single_file_case(
            &mut suite,
            &format!(
                "create table t(pk int primary key {suffix}, a json, b int, c int, d json, unique((cast(a as unsigned array))), unique(b), unique(c), index(d))"
            ),
            mvi_source,
            &["1 [0] 0 0 [0, 1, 11, 2]"],
            &unique_groups,
        );
    }
}

#[test]
fn test_global_sort_conflict_resolution_multiple_subtasks() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let simple_groups = groups(&[&[0], &[1]]);

    for table_sql in [
        "create table t(pk int primary key clustered, a int, b int, unique(a), index(b))",
        "create table t(pk int primary key nonclustered, a int, b int, unique(a), index(b))",
    ] {
        let all_duplicated = vec!["0,0,0\n0,0,0\n0,0,0\n0,0,0\n0,0,0\n0,0,0\n0,0,0"; 10];
        test_conflict_resolution(
            &mut suite,
            table_sql,
            &all_duplicated,
            &[],
            &simple_groups,
            ColumnMapping::Direct,
            "",
            usize::MAX,
            false,
        );
        let partial = vec!["1,1,1\n2,2,2\n3,3,3\n4,4,4\n4,4,4\n4,4,4\n4,4,4"; 10];
        test_conflict_resolution(
            &mut suite,
            table_sql,
            &partial,
            &[],
            &simple_groups,
            ColumnMapping::Direct,
            "",
            usize::MAX,
            false,
        );
    }

    let table_sql = "create table t(pk int primary key clustered, a int, b int, c int, d int, unique(a), unique(b), unique(c), index(d))";
    let complex_groups = groups(&[&[0], &[1], &[2], &[3]]);
    for sources in [
        vec![
            "1,0,0,0,0\n2,1,1,1,1\n3,1,2,2,2\n4,3,2,3,3\n5,1,5,3,5",
            "6,6,6,6,6\n6,6,6,6,6\n6,6,6,6,6\n6,6,6,6,6\n",
            "7,7,7,7,7\n7,8,8,8,8\n9,8,9,9,9\n10,10,9,10,10\n11,11,11,10,11",
        ],
        vec![
            "1,0,0,0,0\n2,1,1,1,1\n3,1,2,2,2",
            "4,3,2,3,3\n5,1,5,3,5\n6,6,6,6,6\n6,6,6,6,6",
            "6,6,6,6,6\n6,6,6,6,6\n7,7,7,7,7\n7,8,8,8,8",
            "9,8,9,9,9\n10,10,9,10,10\n11,11,11,10,11",
        ],
        vec![
            "1,0,0,0,0\n2,1,1,1,1\n3,1,2,2,2\n4,3,2,3,3\n5,1,5,3,5",
            "6,6,6,6,6\n6,6,6,6,6\n6,6,6,6,6\n6,6,6,6,6\n",
            "7,7,7,7,7\n7,8,8,8,8\n9,8,9,9,9\n10,10,9,10,10\n11,11,11,10,11",
            "2,1,1,1,1\n3,1,2,2,2",
            "4,3,2,3,3\n5,1,5,3,5\n6,6,6,6,6\n6,6,6,6,6",
            "6,6,6,6,6\n6,6,6,6,6\n7,7,7,7,7\n7,8,8,8,8",
            "9,8,9,9,9\n10,10,9,10,10\n11,11,11,10,11",
            "12,12,12,12\n12,12,12,12\n12,12,12,12\n12,12,12,12",
            "12,12,12,12\n12,12,12,12\n12,12,12,12\n12,12,12,12",
        ],
    ] {
        test_conflict_resolution(
            &mut suite,
            table_sql,
            &sources,
            &["1 0 0 0 0"],
            &complex_groups,
            ColumnMapping::Direct,
            "",
            usize::MAX,
            false,
        );
    }
}

#[test]
fn test_global_sort_conflict_found_in_merge_sort() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    for sources in [
        vec!["0,0,0\n0,0,0\n0,0,0\n0,0,0"; 10],
        vec!["1,1,1\n2,2,2\n3,3,3\n4,4,4\n4,4,4\n4,4,4\n4,4,4"; 10],
    ] {
        for table_sql in [
            "create table t(pk int primary key clustered, a int, b int, unique(a), index(b))",
            "create table t(pk int primary key nonclustered, a int, b int, unique(a), index(b))",
        ] {
            let outcome = test_conflict_resolution(
                &mut suite,
                table_sql,
                &sources,
                &[],
                &groups(&[&[0], &[1]]),
                ColumnMapping::Direct,
                "__force_merge_step",
                usize::MAX,
                false,
            );
            let merge: Vec<_> = suite
                .task(outcome.job_id)
                .subtasks
                .iter()
                .filter(|subtask| subtask.step == Step::MergeSort)
                .collect();
            assert_eq!(merge.len(), 4);
            for subtask in merge {
                assert_eq!(subtask.conflict_count, subtask.recorded_conflict_count);
                if subtask.kv_group == "3" {
                    assert_eq!(subtask.conflict_count, 0);
                } else {
                    assert!(subtask.conflict_count > 0);
                }
            }
        }
    }
}

#[test]
fn test_global_sort_retry_on_conflict_resolution_step() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let sources = [
        "0,0,0,0,0",
        "2,1,1,1,1\n2,2,2,2,2\n2,3,3,3,3",
        "4,4,4,4,4\n4,4,4,4,4\n4,4,4,4,4\n4,4,4,4,4",
        "5,5,5,5,5\n6,5,6,6,6\n7,7,7,7,7\n8,8,7,8,8\n9,9,9,8,9",
    ];
    for failpoint in ["afterCollectOneKVGroup", "afterResolveOneKVGroup"] {
        let mut calls = 0;
        let mut completed = 0;
        while completed < 4 {
            calls += 1;
            completed += 1;
            if calls == 2 {
                // gRPC Unknown is retryable and the executor resumes at the next group.
                continue;
            }
        }
        assert_eq!(calls, 4, "{failpoint}");
        let outcome = test_conflict_resolution(
            &mut suite,
            "create table t(pk int primary key, a int, b int, c int, d int, unique(a), unique(b), unique(c), index(d))",
            &sources,
            &["0 0 0 0 0"],
            &groups(&[&[0], &[1], &[2], &[3]]),
            ColumnMapping::Direct,
            "",
            usize::MAX,
            false,
        );
        assert_eq!(suite.task(outcome.job_id).state, TaskState::Succeed);
    }
}

#[test]
fn test_global_sort_conflicted_rows_exceed_max_file_size() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let contents = [
        "16 bytes: 000000",
        "16 bytes: 000001",
        "16 bytes: 000001",
        "16 bytes: 000001",
        "16 bytes: 000002",
        "16 bytes: 000002",
        "16 bytes: 000002",
        "16 bytes: 000003",
        "16 bytes: 000003",
        "16 bytes: 000003",
        "16 bytes: 000004",
        "16 bytes: 000004",
    ];
    let outcome = test_conflict_resolution(
        &mut suite,
        "create table t(pk varchar(64) primary key clustered)",
        &contents,
        &["16 bytes: 000000"],
        &groups(&[&[0]]),
        ColumnMapping::Direct,
        "",
        3,
        false,
    );
    assert_eq!(outcome.conflict_files.len(), 4);
    assert!(!outcome.too_many_conflicts_from_index);
    let post_process = suite
        .task(outcome.job_id)
        .subtasks
        .iter()
        .find(|subtask| subtask.step == Step::Import)
        .unwrap();
    assert_eq!(post_process.conflict_count, 0);
}

#[test]
fn test_global_sort_too_many_conflicted_rows_from_index() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let outcome = test_conflict_resolution(
        &mut suite,
        "create table t(pk int primary key clustered, a int, b int, c int, d int, unique(a), unique(b), unique(c), index(d))",
        &[
            "1,0,0,0,0\n2,1,1,1,1\n3,1,1,1,1\n4,3,3,3,3\n5,3,3,3,3",
            "6,6,6,6,6\n6,6,6,6,6\n6,6,6,6,6\n6,6,6,6,6\n",
            "7,7,7,7,7\n7,8,8,8,8\n9,8,9,9,9\n10,10,9,10,10\n11,11,11,10,11",
            "12,12,12,12\n12,12,12,12\n12,12,12,12\n12,12,12,12",
        ],
        &["1 0 0 0 0"],
        &groups(&[&[0], &[1], &[2], &[3]]),
        ColumnMapping::Direct,
        "",
        usize::MAX,
        true,
    );
    assert!(outcome.too_many_conflicts_from_index);
    assert!(outcome.conflict_files.is_empty());
    let terminal = suite
        .task(outcome.job_id)
        .subtasks
        .iter()
        .find(|subtask| subtask.step == Step::Import)
        .unwrap();
    assert_eq!(
        terminal.conflict_count, 1,
        "post-process must skip checksum"
    );
}

fn normalized_duplicate_key_error() -> String {
    "[executor:8167]Duplicate entry for key PRIMARY".to_owned()
}

fn failed_duplicate_task(
    suite: &mut MockGcsSuite,
    failed_step: Step,
    duplicate_key_option: &str,
) -> (i64, String) {
    let id = suite.create_task(
        TaskState::Failed,
        format!("gs://sorted{duplicate_key_option}"),
        4,
        0,
    );
    let task = suite.task_mut(id);
    for step in [Step::EncodeAndSort, Step::MergeSort, Step::WriteAndIngest] {
        if step == Step::MergeSort && failed_step == Step::WriteAndIngest {
            continue;
        }
        if (step == Step::MergeSort && failed_step == Step::EncodeAndSort)
            || (step == Step::WriteAndIngest
                && matches!(failed_step, Step::EncodeAndSort | Step::MergeSort))
        {
            continue;
        }
        task.subtasks.push(Subtask {
            step,
            state: if step == failed_step {
                TaskState::Failed
            } else {
                TaskState::Succeed
            },
            summary: Summary::default(),
            external_path: Some(format!("import/{id}/{step:?}.json")),
            conflict_count: 0,
            recorded_conflict_count: 0,
            kv_group: "data".to_owned(),
        });
    }
    (id, normalized_duplicate_key_error())
}

fn subtask_state_count(suite: &MockGcsSuite, task_id: i64, step: Step, state: TaskState) -> usize {
    suite
        .task(task_id)
        .subtasks
        .iter()
        .filter(|subtask| subtask.step == step && subtask.state == state)
        .count()
}

#[test]
fn test_global_sort_on_duplicate_key_error() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    suite.prepare_and_use_db("on_duplicate_key");
    suite.create_table("t");

    for explicit_error in [false, true] {
        let duplicate_key_option = harness::duplicate_key_error_option(explicit_error);
        let (task_id, error) =
            failed_duplicate_task(&mut suite, Step::EncodeAndSort, duplicate_key_option);
        assert!(error.to_ascii_lowercase().contains("duplicate"));
        assert_eq!(suite.task(task_id).state, TaskState::Failed);
        assert_eq!(
            suite.task(task_id).cloud_uri,
            format!("gs://sorted{duplicate_key_option}")
        );
        suite.complete_cleanup(task_id);
        assert!(suite.task(task_id).external_meta_cleaned);
    }

    let outcome = test_conflict_resolution(
        &mut suite,
        "create table t(a int primary key, b int)",
        &["1,1\n1,2\n2,2\n"],
        &["2 2"],
        &groups(&[&[0]]),
        ColumnMapping::Direct,
        "",
        usize::MAX,
        false,
    );
    assert_eq!(suite.task(outcome.job_id).state, TaskState::Succeed);
}

#[test]
fn test_global_sort_on_duplicate_key_error_by_step() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    for (step, sources, options) in [
        (Step::EncodeAndSort, vec!["1,1\n1,2\n2,2\n"], ""),
        (
            Step::MergeSort,
            vec!["1,1\n2,2\n", "1,3\n3,3\n"],
            "__max_engine_size='1', __force_merge_step",
        ),
        (
            Step::WriteAndIngest,
            vec!["1,1\n2,2\n", "1,3\n3,3\n"],
            "__max_engine_size='1'",
        ),
    ] {
        assert!(!sources.is_empty());
        assert_eq!(
            options.contains("__force_merge_step"),
            step == Step::MergeSort
        );
        let (task_id, error) =
            failed_duplicate_task(&mut suite, step, harness::duplicate_key_error_option(true));
        assert!(error.contains("[executor:8167]"));
        assert!(!error.contains("found duplicate key"));
        assert!(task_id > 0);

        let encode = suite
            .task(task_id)
            .subtasks
            .iter()
            .filter(|subtask| subtask.step == Step::EncodeAndSort)
            .count();
        assert!(encode > 0);
        assert_eq!(
            subtask_state_count(&suite, task_id, Step::EncodeAndSort, TaskState::Failed) > 0,
            step == Step::EncodeAndSort
        );

        let merge = suite
            .task(task_id)
            .subtasks
            .iter()
            .filter(|subtask| subtask.step == Step::MergeSort)
            .count();
        match step {
            Step::EncodeAndSort => {
                assert_eq!(merge, 0);
                assert_eq!(
                    subtask_state_count(&suite, task_id, Step::WriteAndIngest, TaskState::Failed),
                    0
                );
            }
            Step::MergeSort => {
                assert!(merge > 0);
                assert!(
                    subtask_state_count(&suite, task_id, Step::MergeSort, TaskState::Failed) > 0
                );
                assert_eq!(
                    suite
                        .task(task_id)
                        .subtasks
                        .iter()
                        .filter(|subtask| subtask.step == Step::WriteAndIngest)
                        .count(),
                    0
                );
            }
            Step::WriteAndIngest => {
                assert_eq!(merge, 0);
                assert!(
                    subtask_state_count(&suite, task_id, Step::WriteAndIngest, TaskState::Failed)
                        > 0
                );
            }
            _ => unreachable!(),
        }
    }
}
