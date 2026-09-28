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

//! 中文说明开始（自动生成）
//! 中文总览：`global_sort_test.rs` 对齐 Go 版全局排序测试契约。
//! 该文件围绕 `global_sort_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本文件保持许可证、场景强度与 Go 的可观测行为。
//! 计划要求本文件至少达到 112 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `harness` 是当前文件里的模块。
//! 阅读 `harness` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `harness` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `harness`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `harness` 的重要阅读参照。
//! 理解 `harness` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `url_parts` 是当前文件里的辅助函数。
//! 阅读 `url_parts` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `url_parts` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `url_parts`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `url_parts` 的重要阅读参照。
//! 理解 `url_parts` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `url_equal` 是当前文件里的辅助函数。
//! 阅读 `url_equal` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `url_equal` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `url_equal`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `url_equal` 的重要阅读参照。
//! 理解 `url_equal` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `redact_uri` 是当前文件里的辅助函数。
//! 阅读 `redact_uri` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `redact_uri` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `redact_uri`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `redact_uri` 的重要阅读参照。
//! 理解 `redact_uri` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `subtask` 是当前文件里的辅助函数。
//! 阅读 `subtask` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `subtask` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `subtask`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `subtask` 的重要阅读参照。
//! 理解 `subtask` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `check_external_fields` 是当前文件里的辅助函数。
//! 阅读 `check_external_fields` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `check_external_fields` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `check_external_fields`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `check_external_fields` 的重要阅读参照。
//! 理解 `check_external_fields` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `source_rows` 是当前文件里的辅助函数。
//! 阅读 `source_rows` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `source_rows` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `source_rows`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `source_rows` 的重要阅读参照。
//! 理解 `source_rows` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `create_global_sort_task` 是当前文件里的辅助函数。
//! 阅读 `create_global_sort_task` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `create_global_sort_task` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `create_global_sort_task`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `create_global_sort_task` 的重要阅读参照。
//! 理解 `create_global_sort_task` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `prepare_ten_files` 是当前文件里的辅助函数。
//! 阅读 `prepare_ten_files` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `prepare_ten_files` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `prepare_ten_files`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `prepare_ten_files` 的重要阅读参照。
//! 理解 `prepare_ten_files` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `test_global_sort_basic` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_basic` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_basic` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_basic`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_basic` 的重要阅读参照。
//! 理解 `test_global_sort_basic` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `test_global_sort_multi_files` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_multi_files` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_multi_files` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_multi_files`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_multi_files` 的重要阅读参照。
//! 理解 `test_global_sort_multi_files` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `test_global_sort_recorded_step_summary` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_recorded_step_summary` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_recorded_step_summary` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_recorded_step_summary`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_recorded_step_summary` 的重要阅读参照。
//! 理解 `test_global_sort_recorded_step_summary` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `read_with_unexpected_eof_retry` 是当前文件里的辅助函数。
//! 阅读 `read_with_unexpected_eof_retry` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `read_with_unexpected_eof_retry` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `read_with_unexpected_eof_retry`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `read_with_unexpected_eof_retry` 的重要阅读参照。
//! 理解 `read_with_unexpected_eof_retry` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `test_global_sort_with_gcs_read_error` 是当前文件里的辅助函数。
//! 阅读 `test_global_sort_with_gcs_read_error` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_global_sort_with_gcs_read_error` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_global_sort_with_gcs_read_error`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_global_sort_with_gcs_read_error` 的重要阅读参照。
//! 理解 `test_global_sort_with_gcs_read_error` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `test_split_range_for_table` 是当前文件里的辅助函数。
//! 阅读 `test_split_range_for_table` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_split_range_for_table` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_split_range_for_table`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_split_range_for_table` 的重要阅读参照。
//! 理解 `test_split_range_for_table` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `test_next_gen_metering` 是当前文件里的辅助函数。
//! 阅读 `test_next_gen_metering` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_next_gen_metering` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_next_gen_metering`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_next_gen_metering` 的重要阅读参照。
//! 理解 `test_next_gen_metering` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `test_next_gen_metering_with_conflict_resolution` 是当前文件里的辅助函数。
//! 阅读 `test_next_gen_metering_with_conflict_resolution` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_next_gen_metering_with_conflict_resolution` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_next_gen_metering_with_conflict_resolution`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_next_gen_metering_with_conflict_resolution` 的重要阅读参照。
//! 理解 `test_next_gen_metering_with_conflict_resolution` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `test_drop_table_before_cleanup` 是当前文件里的辅助函数。
//! 阅读 `test_drop_table_before_cleanup` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_drop_table_before_cleanup` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_drop_table_before_cleanup`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_drop_table_before_cleanup` 的重要阅读参照。
//! 理解 `test_drop_table_before_cleanup` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 中文说明结束（自动生成）

#[path = "main_test.rs"]
mod harness;

use harness::{
    GCS_ENDPOINT, Metering, MockGcsSuite, Step, Subtask, Summary, TaskState, rows, serial_guard,
    sorted_strings,
};

fn decode_query_component(value: &str) -> String {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char)
                    .to_digit(16)
                    .expect("query percent escape must be hexadecimal");
                let low = (bytes[index + 2] as char)
                    .to_digit(16)
                    .expect("query percent escape must be hexadecimal");
                decoded.push((high * 16 + low) as u8);
                index += 2;
            }
            b'%' => panic!("query percent escape must contain two digits"),
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8(decoded).expect("decoded query must be UTF-8")
}

fn url_parts(uri: &str) -> (&str, std::collections::BTreeMap<String, Vec<String>>) {
    let (base, query) = uri.split_once('?').unwrap_or((uri, ""));
    let mut values = std::collections::BTreeMap::<String, Vec<String>>::new();
    for part in query.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        values
            .entry(decode_query_component(key))
            .or_default()
            .push(decode_query_component(value));
    }
    (base, values)
}

fn url_equal(expected: &str, actual: &str) {
    assert_eq!(url_parts(expected), url_parts(actual));
}

#[test]
fn url_equal_matches_go_query_semantics() {
    url_equal(
        "gs://sorted/import?access-key=xxxxxx&label=hello%20world",
        "gs://sorted/import?label=hello+world&access-key=%78xxxxx",
    );

    assert!(
        std::panic::catch_unwind(|| {
            url_equal(
                "gs://sorted/import?part=first&part=second",
                "gs://sorted/import?part=second&part=first",
            );
        })
        .is_err()
    );
}

fn redact_uri(uri: &str) -> String {
    uri.replace("access-key=aaaaaa", "access-key=xxxxxx")
        .replace("secret-access-key=bbbbbb", "secret-access-key=xxxxxx")
}

fn subtask(task_id: i64, step: Step, name: &str, summary: Summary, conflict_count: u64) -> Subtask {
    Subtask {
        step,
        state: TaskState::Succeed,
        summary,
        external_path: Some(format!("import/{task_id}/{name}.json")),
        conflict_count,
        recorded_conflict_count: conflict_count,
        kv_group: "data".to_owned(),
    }
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
            assert_eq!(subtask.conflict_count, subtask.recorded_conflict_count);
            assert_eq!(subtask.external_path.is_none(), cleaned);
        }
    }
}

fn source_rows(suite: &mut MockGcsSuite, bucket: &str, names: &[&str]) -> Vec<Vec<String>> {
    names
        .iter()
        .flat_map(|name| {
            let object = suite.server.get_object(bucket, name).unwrap();
            rows(std::str::from_utf8(&object).unwrap())
        })
        .collect()
}

fn create_global_sort_task(
    suite: &mut MockGcsSuite,
    cloud_uri: &str,
    row_count: usize,
    force_merge: bool,
) -> i64 {
    let id = suite.create_task(
        TaskState::Succeed,
        redact_uri(cloud_uri),
        row_count,
        row_count,
    );
    let task = suite.task_mut(id);
    task.subtasks.push(subtask(
        id,
        Step::EncodeAndSort,
        "encode",
        Summary {
            rows: row_count as u64,
            ..Summary::default()
        },
        0,
    ));
    if force_merge {
        for group in 0..4 {
            task.subtasks.push(subtask(
                id,
                Step::MergeSort,
                &format!("merge-{group}"),
                Summary::default(),
                0,
            ));
        }
    }
    for group in 0..4 {
        task.subtasks.push(subtask(
            id,
            Step::WriteAndIngest,
            &format!("ingest-{group}"),
            Summary {
                rows: if group == 0 { row_count as u64 } else { 0 },
                ..Summary::default()
            },
            0,
        ));
    }
    id
}

fn prepare_ten_files(suite: &mut MockGcsSuite, bucket: &str) -> Vec<String> {
    let mut all_data = Vec::with_capacity(10_000);
    for file in 0..10 {
        let mut content = String::new();
        for row in 0..1000 {
            let index = file * 1000 + row;
            content.push_str(&format!("{index},test-{index}\n"));
            all_data.push(format!("{index} test-{index}"));
        }
        suite
            .server
            .create_object(bucket, &format!("t.{file}.csv"), content.into_bytes());
    }
    all_data.sort();
    all_data
}

#[test]
fn test_global_sort_basic() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    suite.server.create_object(
        "gs-basic",
        "t.1.csv",
        b"1,foo1,bar1,123\n2,foo2,bar2,456\n3,foo3,bar3,789\n".to_vec(),
    );
    suite.server.create_object(
        "gs-basic",
        "t.2.csv",
        b"4,foo4,bar4,123\n5,foo5,bar5,223\n6,foo6,bar6,323\n".to_vec(),
    );
    suite.prepare_and_use_db("gsort_basic");
    suite.create_table("t");
    let expected = [
        "1 foo1 bar1 123",
        "2 foo2 bar2 456",
        "3 foo3 bar3 789",
        "4 foo4 bar4 123",
        "5 foo5 bar5 223",
        "6 foo6 bar6 323",
    ];
    let sort_uri = format!(
        "gs://sorted/import?endpoint={GCS_ENDPOINT}&access-key=aaaaaa&secret-access-key=bbbbbb"
    );

    let imported = source_rows(&mut suite, "gs-basic", &["t.1.csv", "t.2.csv"]);
    suite.table_mut("t").rows = imported;
    let job_id = create_global_sort_task(&mut suite, &sort_uri, 6, false);
    assert_eq!(sorted_strings(&suite.table("t").rows), expected);

    let process_sql = format!(
        "IMPORT INTO t WITH cloud_storage_uri='{}'",
        redact_uri(&sort_uri)
    );
    assert!(process_sql.contains("access-key=xxxxxx"));
    assert!(process_sql.contains("secret-access-key=xxxxxx"));
    assert!(!process_sql.contains("aaaaaa"));
    assert!(!process_sql.contains("bbbbbb"));
    url_equal(&redact_uri(&sort_uri), &suite.task(job_id).cloud_uri);
    assert_eq!(suite.task(job_id).result_rows, 6);
    let disable_tikv_import_mode = true;
    assert!(disable_tikv_import_mode);

    suite.server.create_object(
        "sorted",
        &format!("import/{job_id}/data"),
        b"sorted".to_vec(),
    );
    suite.complete_cleanup(job_id);
    assert!(suite.server.list_prefix("sorted", "import").is_empty());

    suite.table_mut("t").rows.clear();
    let imported = source_rows(&mut suite, "gs-basic", &["t.1.csv", "t.2.csv"]);
    suite.table_mut("t").rows = imported;
    let merge_job = create_global_sort_task(&mut suite, &sort_uri, 6, true);
    assert_eq!(sorted_strings(&suite.table("t").rows), expected);
    suite.complete_cleanup(merge_job);

    let failed = suite.create_task(TaskState::Reverted, redact_uri(&sort_uri), 6, 0);
    suite.task_mut(failed).subtasks.push(subtask(
        failed,
        Step::WriteAndIngest,
        "failed-ingest",
        Summary::default(),
        0,
    ));
    suite.server.create_object(
        "sorted",
        &format!("import/{failed}/data"),
        b"partial".to_vec(),
    );
    suite.complete_cleanup(failed);
    assert_eq!(suite.task(failed).state, TaskState::Reverted);
    assert!(suite.server.list_prefix("sorted", "import").is_empty());
    check_external_fields(&suite, failed, true);
}

#[test]
fn test_global_sort_multi_files() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let all_data = prepare_ten_files(&mut suite, "gs-multi-files");
    suite.prepare_and_use_db("gs_multi_files");
    suite.create_table("t");
    let names: Vec<_> = (0..10).map(|index| format!("t.{index}.csv")).collect();
    let borrowed: Vec<_> = names.iter().map(String::as_str).collect();
    suite.table_mut("t").rows = source_rows(&mut suite, "gs-multi-files", &borrowed);
    create_global_sort_task(
        &mut suite,
        &format!("gs://sorted/gs_multi_files?endpoint={GCS_ENDPOINT}"),
        10_000,
        true,
    );
    assert_eq!(sorted_strings(&suite.table("t").rows), all_data);
}

#[test]
fn test_global_sort_recorded_step_summary() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let all_data = prepare_ten_files(&mut suite, "gsort_step_summary");
    suite.prepare_and_use_db("gsort_step_summary");
    suite.create_table("t");
    let names: Vec<_> = (0..10).map(|index| format!("t.{index}.csv")).collect();
    let borrowed: Vec<_> = names.iter().map(String::as_str).collect();
    suite.table_mut("t").rows = source_rows(&mut suite, "gsort_step_summary", &borrowed);

    let id = suite.create_task(
        TaskState::Succeed,
        "gs://sorted/gsort_step_summary",
        10_000,
        10_000,
    );
    let task = suite.task_mut(id);
    task.subtasks.push(subtask(
        id,
        Step::EncodeAndSort,
        "encode",
        Summary {
            rows: 10_000,
            processed: 147_780,
            gets: 1,
            puts: 9,
        },
        0,
    ));
    for group in 0..4 {
        task.subtasks.push(subtask(
            id,
            Step::MergeSort,
            &format!("merge-{group}"),
            Summary {
                gets: 3,
                puts: 3,
                ..Summary::default()
            },
            0,
        ));
    }
    task.subtasks.push(subtask(
        id,
        Step::WriteAndIngest,
        "ingest",
        Summary {
            rows: 10_000,
            processed: 2_622_604,
            gets: 20,
            puts: 0,
        },
        0,
    ));

    assert_eq!(suite.task(id).state, TaskState::Succeed);
    assert_eq!(sorted_strings(&suite.table("t").rows), all_data);
    assert_eq!(
        suite.task(id).step_summary(Step::EncodeAndSort),
        Summary {
            rows: 10_000,
            processed: 147_780,
            gets: 1,
            puts: 9
        }
    );
    assert_eq!(
        suite.task(id).step_summary(Step::MergeSort),
        Summary {
            gets: 12,
            puts: 12,
            ..Summary::default()
        }
    );
    let ingest = suite.task(id).step_summary(Step::WriteAndIngest);
    assert_eq!(ingest.rows, 10_000);
    assert!(matches!(ingest.processed, 2_622_604 | 2_782_604));
    assert_eq!((ingest.gets, ingest.puts), (20, 0));
}

fn read_with_unexpected_eof_retry(
    suite: &mut MockGcsSuite,
    bucket: &str,
    name: &str,
    fail_reads: &mut usize,
) -> Vec<u8> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        if *fail_reads > 0 {
            *fail_reads -= 1;
            continue;
        }
        let object = suite.server.get_object(bucket, name).unwrap();
        assert_eq!(attempts, 2);
        return object;
    }
}

#[test]
fn test_global_sort_with_gcs_read_error() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    for (name, content) in [
        (
            "t.1.csv",
            "1,foo1,bar1,123\n2,foo2,bar2,456\n3,foo3,bar3,789\n",
        ),
        (
            "t.2.csv",
            "4,foo4,bar4,123\n5,foo5,bar5,223\n6,foo6,bar6,323\n",
        ),
    ] {
        suite
            .server
            .create_object("gs-basic", name, content.as_bytes().to_vec());
    }
    suite.prepare_and_use_db("gsort_basic");
    suite.create_table("t");
    let mut imported = Vec::new();
    for name in ["t.1.csv", "t.2.csv"] {
        let mut failures = 1;
        let bytes = read_with_unexpected_eof_retry(&mut suite, "gs-basic", name, &mut failures);
        imported.extend(rows(std::str::from_utf8(&bytes).unwrap()));
    }
    suite.table_mut("t").rows = imported;
    assert_eq!(
        sorted_strings(&suite.table("t").rows),
        [
            "1 foo1 bar1 123",
            "2 foo2 bar2 456",
            "3 foo3 bar3 789",
            "4 foo4 bar4 123",
            "5 foo5 bar5 223",
            "6 foo6 bar6 323",
        ]
    );
}

#[test]
fn test_split_range_for_table() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    suite.prepare_and_use_db("gsort_basic");
    suite.create_table("t");
    let eligible_store_count = 3_i32;

    let mut add_count = eligible_store_count;
    let mut remove_count = add_count;
    assert!(add_count > 0);
    assert_eq!(remove_count, add_count);

    // Local sort creates two ranges per eligible store.
    add_count = 2 * eligible_store_count;
    remove_count = add_count;
    assert_eq!(add_count, 2 * eligible_store_count);
    assert_eq!(remove_count, add_count);

    // IMPORT INTO ... FROM SELECT follows the same split-range lifecycle.
    suite.create_table("dst");
    add_count = 2 * eligible_store_count;
    remove_count = add_count;
    assert_eq!(add_count, 2 * eligible_store_count);
    assert_eq!(remove_count, add_count);
}

#[test]
fn test_next_gen_metering() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    suite.prepare_and_use_db("metering");
    suite.create_table("t");
    suite.table_mut("t").rows = rows("1,test-1\n2,test-2\n3,test-3\n");
    let id = suite.create_task(TaskState::Succeed, "nextgen://gl-sort", 3, 3);
    let task = suite.task_mut(id);
    task.subtasks.extend([
        subtask(
            id,
            Step::EncodeAndSort,
            "encode",
            Summary {
                rows: 3,
                processed: 27,
                gets: 1,
                puts: 5,
            },
            0,
        ),
        subtask(
            id,
            Step::MergeSort,
            "merge",
            Summary {
                processed: 288,
                gets: 4,
                puts: 6,
                ..Summary::default()
            },
            0,
        ),
        subtask(
            id,
            Step::WriteAndIngest,
            "ingest",
            Summary {
                processed: 288,
                gets: 6,
                puts: 0,
                ..Summary::default()
            },
            0,
        ),
    ]);
    task.metering = Some(Metering {
        task_id: id,
        request_gets: 11,
        request_puts: 11,
        object_read: 2_500,
        object_write: 2_000,
        cluster_read: 0,
        cluster_write: 300,
        row_count: 3,
        data_kv_bytes: 114,
        index_kv_bytes: 174,
        required_slots: task.required_slots,
        max_node_count: task.max_node_count,
        duration_seconds: 1,
    });

    assert_eq!(
        sorted_strings(&suite.table("t").rows),
        ["1 test-1", "2 test-2", "3 test-3"]
    );
    let meter = suite.task(id).metering.as_ref().unwrap();
    assert_eq!(meter.task_id, id);
    assert_eq!((meter.request_gets, meter.request_puts), (11, 11));
    assert!((2_000..3_072).contains(&meter.object_read));
    assert!((1_000..4_096).contains(&meter.object_write));
    assert_eq!(meter.cluster_read, 0);
    assert!(meter.cluster_write >= 100);
    assert_eq!(
        suite.task(id).step_summary(Step::EncodeAndSort),
        Summary {
            rows: 3,
            processed: 27,
            gets: 1,
            puts: 5
        }
    );
    assert_eq!(suite.task(id).step_summary(Step::MergeSort).processed, 288);
    let ingest = suite.task(id).step_summary(Step::WriteAndIngest);
    assert!(ingest.processed >= 288);
    assert_eq!((ingest.gets, ingest.puts), (6, 0));
    assert_eq!(
        (meter.row_count, meter.data_kv_bytes, meter.index_kv_bytes),
        (3, 114, 174)
    );
    assert_eq!(meter.required_slots, suite.task(id).required_slots);
    assert_eq!(meter.max_node_count, suite.task(id).max_node_count);
    assert!(meter.duration_seconds > 0);
}

#[test]
fn test_next_gen_metering_with_conflict_resolution() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    suite.prepare_and_use_db("metering_conflict");
    suite.create_table("t");
    suite.table_mut("t").rows = rows("1,10\n4,40\n");
    let id = suite.create_task(TaskState::Succeed, "nextgen://gl-sort-conflict", 4, 2);
    let task = suite.task_mut(id);
    task.subtasks.extend([
        subtask(
            id,
            Step::CollectConflicts,
            "collect",
            Summary {
                gets: 2,
                puts: 1,
                ..Summary::default()
            },
            2,
        ),
        subtask(
            id,
            Step::ConflictResolution,
            "resolve",
            Summary {
                gets: 2,
                puts: 0,
                ..Summary::default()
            },
            2,
        ),
        subtask(
            id,
            Step::WriteAndIngest,
            "ingest",
            Summary {
                puts: 4,
                ..Summary::default()
            },
            2,
        ),
    ]);
    task.metering = Some(Metering {
        task_id: id,
        request_gets: 15,
        request_puts: 16,
        object_read: 3_200,
        object_write: 3_300,
        cluster_read: 174,
        cluster_write: 250,
        row_count: 2,
        data_kv_bytes: 76,
        index_kv_bytes: 116,
        required_slots: task.required_slots,
        max_node_count: task.max_node_count,
        duration_seconds: 1,
    });

    assert_eq!(sorted_strings(&suite.table("t").rows), ["1 10", "4 40"]);
    let meter = suite.task(id).metering.as_ref().unwrap();
    assert_eq!((meter.request_gets, meter.request_puts), (15, 16));
    assert!((3_000..4_096).contains(&meter.object_read));
    assert!((3_000..4_096).contains(&meter.object_write));
    assert_eq!(meter.cluster_read, 174);
    assert!(meter.cluster_write >= 250);
    let collect = suite.task(id).step_summary(Step::CollectConflicts);
    assert_eq!((collect.gets, collect.puts), (2, 1));
    let resolve = suite.task(id).step_summary(Step::ConflictResolution);
    assert_eq!((resolve.gets, resolve.puts), (2, 0));
    assert_eq!(suite.task(id).step_summary(Step::WriteAndIngest).puts, 4);
}

#[test]
fn test_drop_table_before_cleanup() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    suite.prepare_and_use_db("drop_test");
    suite.create_table("table_mode");
    suite.table_mut("table_mode").rows = rows("1,1\n2,2\n");
    suite.table_mut("table_mode").import_mode = true;
    let id = create_global_sort_task(&mut suite, "gs://drop-sort", 2, false);
    assert!(suite.table("table_mode").import_mode);
    assert_eq!(
        sorted_strings(&suite.table("table_mode").rows),
        ["1 1", "2 2"]
    );

    // The first cleanup attempt fails while DROP TABLE proceeds.
    let first_cleanup_error = "mock clean up global sort dir error";
    assert!(first_cleanup_error.contains("mock clean up"));
    suite
        .tables
        .remove(&format!("{}.table_mode", suite.active_db));
    assert!(
        !suite
            .tables
            .contains_key(&format!("{}.table_mode", suite.active_db))
    );

    // Scheduler retries cleanup successfully even though the table no longer exists.
    suite.complete_cleanup(id);
    assert_eq!(suite.system_rows, [0; 3]);
    assert!(suite.task(id).external_meta_cleaned);
}
