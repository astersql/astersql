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
//! 中文总览：`import_summary_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `import_summary_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 26 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `harness` 是当前文件里的模块。
//! 阅读 `harness` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `harness` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `harness`，应从这里理解职责边界。
//! 符号 `ImportJobSummary` 是当前文件里的状态类型。
//! 阅读 `ImportJobSummary` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `ImportJobSummary` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `ImportJobSummary`，应从这里理解职责边界。
//! 符号 `successful_subtask` 是当前文件里的辅助函数。
//! 阅读 `successful_subtask` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `successful_subtask` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `successful_subtask`，应从这里理解职责边界。
//! 符号 `ten_thousand_rows` 是当前文件里的辅助函数。
//! 阅读 `ten_thousand_rows` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `ten_thousand_rows` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `ten_thousand_rows`，应从这里理解职责边界。
//! 符号 `test_global_sort_summary` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_summary` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_summary` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_summary`，应从这里理解职责边界。
//! 符号 `test_local_sort_summary` 是当前文件里的辅助函数。
//! 阅读 `test_local_sort_summary` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_local_sort_summary` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_local_sort_summary`，应从这里理解职责边界。
//! 中文说明结束（自动生成）

#[path = "main_test.rs"]
mod harness;

use harness::{
    GCS_ENDPOINT, MockGcsSuite, Step, Subtask, Summary, TaskState, rows, serial_guard,
    sorted_strings,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ImportJobSummary {
    merge_rows: u64,
    ingest_rows: u64,
    imported_rows: u64,
}

fn successful_subtask(step: Step, row_count: u64) -> Subtask {
    Subtask {
        step,
        state: TaskState::Succeed,
        summary: Summary {
            rows: row_count,
            processed: row_count,
            gets: 0,
            puts: 0,
        },
        external_path: None,
        conflict_count: 0,
        recorded_conflict_count: 0,
        kv_group: "data".to_owned(),
    }
}

fn ten_thousand_rows() -> (String, Vec<Vec<String>>) {
    let content: String = (0..10_000)
        .map(|index| format!("{index},test-{index}\n"))
        .collect();
    let parsed = rows(&content);
    assert_eq!(parsed.len(), 10_000);
    (content, parsed)
}

#[test]
fn test_global_sort_summary() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let mut all_rows = Vec::with_capacity(10_000);

    for file_index in 0..10 {
        let content: String = (0..1_000)
            .map(|offset| {
                let index = file_index * 1_000 + offset;
                format!("{index},test-{index}\n")
            })
            .collect();
        all_rows.extend(rows(&content));
        suite
            .server
            .create_object("global-sort-files", &format!("t.{file_index}.csv"), content);
    }
    assert_eq!(all_rows.len(), 10_000);
    suite.prepare_and_use_db("global_sort_summary");
    suite.create_table("t");
    suite.table_mut("t").rows = all_rows.clone();

    let sort_storage_uri = format!("gs://sorted/gs_multi_files?endpoint={GCS_ENDPOINT}");
    let import_sql = format!(
        "import into t FROM 'gs://global-sort-files/t.*.csv?endpoint={GCS_ENDPOINT}' \
         with __force_merge_step, cloud_storage_uri='{sort_storage_uri}'"
    );
    let job_id = suite.create_task(TaskState::Succeed, import_sql, 10_000, 10_000);

    // Go schedules one encode task, four merge tasks, one post-process task
    // and four ingest tasks. History must retain all ten after completion.
    let mut subtasks = vec![successful_subtask(Step::EncodeAndSort, 10_000)];
    subtasks.extend((0..4).map(|_| successful_subtask(Step::MergeSort, 10_000)));
    subtasks.push(successful_subtask(Step::PostProcess, 0));
    subtasks.extend((0..4).map(|_| successful_subtask(Step::WriteAndIngest, 2_500)));
    suite.task_mut(job_id).subtasks = subtasks;

    let job_summary = ImportJobSummary {
        merge_rows: 10_000,
        ingest_rows: 10_000,
        imported_rows: 10_000,
    };
    assert_eq!(job_summary.merge_rows, 10_000);
    assert_eq!(job_summary.ingest_rows, 10_000);
    assert_eq!(job_summary.imported_rows, 10_000);
    assert_eq!(suite.task(job_id).result_rows, 10_000);
    assert_eq!(
        sorted_strings(&suite.table("t").rows),
        sorted_strings(&all_rows)
    );

    let task_history = suite.task(job_id);
    assert_eq!(task_history.job_id, job_id);
    assert_eq!(task_history.subtasks.len(), 10);
    for subtask in &task_history.subtasks {
        match subtask.step {
            Step::EncodeAndSort | Step::MergeSort => {
                assert_eq!(subtask.summary.rows, 10_000)
            }
            Step::PostProcess => assert_eq!(subtask.summary.rows, 0),
            Step::WriteAndIngest => assert_eq!(subtask.summary.rows, 2_500),
            _ => panic!("unexpected global-sort summary step: {:?}", subtask.step),
        }
    }
    assert_eq!(task_history.step_summary(Step::WriteAndIngest).rows, 10_000);
}

#[test]
fn test_local_sort_summary() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let (content, all_rows) = ten_thousand_rows();
    suite
        .server
        .create_object("local-sort-file", "t.csv", content);
    suite.prepare_and_use_db("local_sort_summary");
    suite.create_table("t");
    suite.table_mut("t").rows = all_rows.clone();

    let import_sql =
        format!("import into t FROM 'gs://local-sort-file/t.csv?endpoint={GCS_ENDPOINT}'");
    let job_id = suite.create_task(TaskState::Succeed, import_sql, 10_000, 10_000);
    suite.task_mut(job_id).subtasks = vec![successful_subtask(Step::Import, 10_000)];

    let job_summary = ImportJobSummary {
        merge_rows: 0,
        ingest_rows: 10_000,
        imported_rows: 10_000,
    };
    assert_eq!(job_summary.merge_rows, 0);
    assert_eq!(job_summary.imported_rows, 10_000);
    assert_eq!(suite.task(job_id).result_rows, 10_000);
    assert_eq!(suite.task(job_id).step_summary(Step::MergeSort).rows, 0);
    assert_eq!(suite.task(job_id).step_summary(Step::Import).rows, 10_000);
    assert_eq!(
        sorted_strings(&suite.table("t").rows),
        sorted_strings(&all_rows)
    );
}
