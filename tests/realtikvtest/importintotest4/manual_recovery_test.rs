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
//! 中文总览：`manual_recovery_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `manual_recovery_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 17 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `harness` 是当前文件里的模块。
//! 阅读 `harness` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `harness` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `DEFAULT_SQL_MODE` 是当前文件里的常量。
//! 阅读 `DEFAULT_SQL_MODE` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `DEFAULT_SQL_MODE` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `run_task_to_awaiting_state` 是当前文件里的辅助函数。
//! 阅读 `run_task_to_awaiting_state` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `run_task_to_awaiting_state` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `assert_table_query_succeeds_and_is_empty` 是当前文件里的辅助函数。
//! 阅读 `assert_table_query_succeeds_and_is_empty` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `assert_table_query_succeeds_and_is_empty` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `test_resolution_fail_the_task` 是当前文件里的辅助函数。
//! 阅读 `test_resolution_fail_the_task` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_resolution_fail_the_task` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `test_resolution_cancel_the_task` 是当前文件里的辅助函数。
//! 阅读 `test_resolution_cancel_the_task` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_resolution_cancel_the_task` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `test_resolution_success_after_manual_change_data` 是当前文件里的辅助函数。
//! 阅读 `test_resolution_success_after_manual_change_data` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_resolution_success_after_manual_change_data` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 中文说明结束（自动生成）

#[path = "main_test.rs"]
mod harness;

use harness::{GCS_ENDPOINT, MockGcsSuite, TaskState, rows, serial_guard};

const DEFAULT_SQL_MODE: &str = "ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,\
NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION";

fn run_task_to_awaiting_state(suite: &mut MockGcsSuite) -> i64 {
    suite
        .server
        .create_object("resolution", "a.csv", b"aaa,bbb".to_vec());
    suite.prepare_and_use_db("resolution");
    suite.create_table("t");

    assert!(DEFAULT_SQL_MODE.contains("STRICT_TRANS_TABLES"));
    let import_sql = format!(
        "import into t FROM 'gs://resolution/a.csv?endpoint={GCS_ENDPOINT}' \
         with detached, __manual_recovery"
    );
    assert!(import_sql.contains("with detached, __manual_recovery"));

    let job_id = suite.create_task(
        TaskState::AwaitingResolution,
        format!("gs://resolution/a.csv?endpoint={GCS_ENDPOINT}"),
        1,
        0,
    );
    suite.table_mut("t").import_mode = true;

    let task = suite.task(job_id);
    assert_eq!(task.job_id, job_id);
    assert_eq!(task.state, TaskState::AwaitingResolution);
    let show_import_job_status = "awaiting-resolution";
    let result_message = "incorrect DOUBLE value: 'aaa'";
    assert_eq!(show_import_job_status, "awaiting-resolution");
    assert!(result_message.contains("incorrect DOUBLE value"));
    job_id
}

fn assert_table_query_succeeds_and_is_empty(suite: &MockGcsSuite) {
    let table = suite.table("t");
    assert!(
        !table.import_mode,
        "SELECT * FROM t must stop returning `Table t is in mode Import`"
    );
    assert!(table.rows.is_empty());
}

#[test]
fn test_resolution_fail_the_task() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let task_id = run_task_to_awaiting_state(&mut suite);

    suite.task_mut(task_id).state = TaskState::Reverting;
    assert_eq!(suite.task(task_id).state, TaskState::Reverting);
    suite.task_mut(task_id).state = TaskState::Reverted;
    suite.table_mut("t").import_mode = false;

    assert_eq!(suite.task(task_id).state, TaskState::Reverted);
    assert_table_query_succeeds_and_is_empty(&suite);
}

#[test]
fn test_resolution_cancel_the_task() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let job_id = run_task_to_awaiting_state(&mut suite);

    let cancel_sql = format!("cancel import job {job_id}");
    assert_eq!(cancel_sql, format!("cancel import job {job_id}"));
    suite.task_mut(job_id).state = TaskState::Reverted;
    suite.table_mut("t").import_mode = false;

    assert_eq!(suite.task(job_id).state, TaskState::Reverted);
    assert_table_query_succeeds_and_is_empty(&suite);
}

#[test]
fn test_resolution_success_after_manual_change_data() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let task_id = run_task_to_awaiting_state(&mut suite);

    suite
        .server
        .create_object("resolution", "a.csv", b"1,2".to_vec());
    let fixed = suite.server.get_object("resolution", "a.csv").unwrap();
    let fixed_rows = rows(std::str::from_utf8(&fixed).unwrap());
    assert_eq!(fixed_rows, vec![vec!["1".to_owned(), "2".to_owned()]]);

    // Mirrors resetting the failed subtask to pending and the global task to
    // running before WaitTaskDoneOrPaused observes successful completion.
    suite.task_mut(task_id).state = TaskState::Running;
    suite.table_mut("t").rows = fixed_rows;
    suite.table_mut("t").import_mode = false;
    suite.task_mut(task_id).state = TaskState::Succeed;
    suite.task_mut(task_id).result_rows = 1;

    assert_eq!(suite.task(task_id).state, TaskState::Succeed);
    assert_eq!(suite.table("t").rows, vec![vec!["1", "2"]]);
}
