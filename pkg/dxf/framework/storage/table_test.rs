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

// DXF（Distributed eXecution Framework，分布式执行框架）任务表存储层的单元测试。
//
// 验证 `TaskManager` 对全局任务表（`mysql.tidb_global_task`）与子任务表
// （`mysql.tidb_background_subtask`）的查询、切步、槽位统计等 SQL 路径，
// 以及任务不存在、事务 entry size 限制等边界行为。

use crate::*;
use std::time::UNIX_EPOCH;

/// 构造一条任务基行列（不含 meta/error 等扩展列），便于 mock 查询结果。
fn task_base_row(id: i64, state: &str) -> chunk::Row {
    chunk::Row::new(vec![
        Cell::Int(id),
        Cell::String(format!("task-{id}")),
        Cell::String("Example".into()),
        Cell::String(state.into()),
        Cell::Int(proto::StepOne),
        Cell::Int(100),
        Cell::Int(8),
        Cell::Time(UNIX_EPOCH),
        Cell::String("background".into()),
        Cell::Int(2),
        Cell::Json("{}".into()),
        Cell::String("ks1".into()),
    ])
}

/// 构造用于切步等写路径测试的完整任务对象。
fn task(id: i64, state: proto::TaskState, step: proto::Step) -> proto::Task {
    proto::Task {
        TaskBase: proto::TaskBase {
            ID: id,
            Key: format!("task-{id}"),
            Type: proto::TaskTypeExample,
            State: state,
            Step: step,
            RequiredSlots: 4,
            ..proto::TaskBase::default()
        },
        Meta: vec![1, 2, 3],
        ..proto::Task::default()
    }
}

/// 断言操作返回期望的 Go 哨兵错误（如 ErrTaskNotFound）。
fn assert_error<T>(result: Result<T, Error>, expected: GoError) {
    match result {
        Err(error) => assert_eq!(error, expected),
        Ok(_) => panic!("expected {expected:?}"),
    }
}

#[test]
/// 按 ID 读取任务基行，并校验不存在时返回 ErrTaskNotFound 及 SQL 参数。
fn TestTaskTable() {
    let manager = TaskManager::new();
    manager.push_result(vec![task_base_row(7, proto::TaskStateRunning)]);
    let loaded = manager.GetTaskBaseByID((), 7).expect("task row");
    assert_eq!(loaded.ID, 7);
    assert_eq!(loaded.State, proto::TaskStateRunning);
    assert_eq!(loaded.RequiredSlots, 8);

    assert_error(manager.GetTaskByID((), 8), ErrTaskNotFound);
    let calls = manager.calls();
    assert!(calls[0].sql.contains("where id = %?"));
    assert_eq!(calls[0].args, vec![Value::Int(7)]);
}

#[test]
/// 校验按 keyspace（逻辑租户/命名空间）汇总活跃任务数。
fn TestGetActiveTaskCountsByKeyspace() {
    let manager = TaskManager::new();
    let empty = manager.GetActiveTaskCountsByKeyspace(()).unwrap();
    assert_eq!(empty.Total, 0);
    assert!(empty.PerKeyspace.is_empty());

    manager.push_result(vec![
        chunk::Row::new(vec![Cell::String("ks1".into()), Cell::Int(2)]),
        chunk::Row::new(vec![Cell::String("ks2".into()), Cell::Int(3)]),
    ]);
    let summary = manager.GetActiveTaskCountsByKeyspace(()).unwrap();
    assert_eq!(summary.Total, 5);
    assert_eq!(summary.PerKeyspace.get("ks1"), Some(&2));
    assert_eq!(summary.PerKeyspace.get("ks2"), Some(&3));
}

#[test]
/// 校验切步：更新任务状态/步骤并批量插入新 subtask。
fn TestSwitchTaskStep() {
    let manager = TaskManager::new();
    manager.set_affected_rows(1);
    let mut subtask = proto::Subtask::for_test(11, vec![9]);
    subtask.TaskID = 7;
    subtask.Step = 2;
    subtask.ExecID = "node-1".into();
    subtask.Type = proto::TaskTypeExample;
    subtask.Concurrency = 4;
    manager
        .SwitchTaskStep(
            (),
            task(7, proto::TaskStatePending, proto::StepInit),
            proto::TaskStateRunning,
            2,
            vec![subtask],
        )
        .unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].sql.contains("start_time = CURRENT_TIMESTAMP()"));
    assert!(
        calls[1]
            .sql
            .contains("insert into mysql.tidb_background_subtask")
    );
    assert_eq!(calls[1].args.len(), 8);

    // Go 在 CAS 更新不到行时将其视为另一 scheduler 已完成切步，跳过 subtask 插入。
    let manager = TaskManager::new();
    manager.set_affected_rows(0);
    manager
        .SwitchTaskStep(
            (),
            task(8, proto::TaskStatePending, proto::StepInit),
            proto::TaskStateRunning,
            proto::StepOne,
            vec![proto::Subtask::for_test(12, vec![1])],
        )
        .unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0]
            .sql
            .contains("where id = %? and state = %? and step = %?")
    );
}

#[test]
/// 校验按 step 读取子任务 summary（JSON，含 row_count 等进度字段）。
fn TestGetSubtaskSummaries() {
    let manager = TaskManager::new();
    assert!(
        manager
            .GetAllSubtaskSummaryByStep((), 7, 2)
            .unwrap()
            .is_none()
    );

    manager.push_result(vec![
        chunk::Row::new(vec![Cell::Json(r#"{"row_count":12}"#.into())]),
        chunk::Row::new(vec![Cell::Json(r#"{"row_count":8}"#.into())]),
    ]);
    let summaries = manager
        .GetAllSubtaskSummaryByStep((), 7, 2)
        .unwrap()
        .expect("summaries");
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].RowCount, 12);
    assert_eq!(summaries[1].RowCount, 8);

    manager.push_result(vec![chunk::Row::new(vec![Cell::Json("{".into())])]);
    assert!(manager.GetAllSubtaskSummaryByStep((), 7, 2).is_err());
}

#[test]
/// 校验节点已用 slot（槽位，最小资源粒度）统计与忙碌节点列表。
fn TestGetUsedSlotsOnNodesAndBusyNodes() {
    let manager = TaskManager::new();
    manager.push_result(vec![
        chunk::Row::new(vec![Cell::String("n1".into()), Cell::Decimal(7)]),
        chunk::Row::new(vec![Cell::String("n2".into()), Cell::Decimal(11)]),
    ]);
    let slots = manager.GetUsedSlotsOnNodes(()).unwrap();
    assert_eq!(slots.get("n1"), Some(&7));
    assert_eq!(slots.get("n2"), Some(&11));

    manager.push_result(vec![
        chunk::Row::new(vec![Cell::String("n1".into())]),
        chunk::Row::new(vec![Cell::String("n2".into())]),
    ]);
    let busy = manager.GetBusyNodes(()).unwrap();
    assert_eq!(
        busy.iter().map(|node| node.ID.as_str()).collect::<Vec<_>>(),
        vec!["n1", "n2"]
    );
}

#[test]
/// 校验将 running 子任务回退为 pending（如节点故障后重新调度）。
fn TestRunningSubtasksBack2Pending() {
    let manager = TaskManager::new();
    manager.RunningSubtasksBack2Pending((), vec![]).unwrap();
    assert!(manager.calls().is_empty());

    let mut first = proto::SubtaskBase::default();
    first.ID = 1;
    first.ExecID = "n1".into();
    let mut second = proto::SubtaskBase::default();
    second.ID = 2;
    second.ExecID = "n2".into();
    manager
        .RunningSubtasksBack2Pending((), vec![first, second])
        .unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].args[1], Value::Int(1));
    assert_eq!(calls[1].args[2], Value::String("n2".into()));
    assert!(calls.iter().all(|call| call.sql.contains("state = %?")));
}

#[test]
/// 校验按 ID/Key 查询缺失任务均映射为 ErrTaskNotFound。
fn TestTaskNotFound() {
    let manager = TaskManager::new();
    assert_error(manager.GetTaskBaseByID((), 404), ErrTaskNotFound);
    assert_error(manager.GetTaskByKey((), "missing".into()), ErrTaskNotFound);
}

#[test]
/// 校验 WithNewSession 内可临时调整事务 entry size 限制并恢复。
fn TestTaskManagerEntrySize() {
    let manager = TaskManager::new();
    manager
        .WithNewSession(|session| {
            let before = session.TxnEntrySizeLimit();
            session.SetTxnEntrySizeLimit(123);
            assert_eq!(session.TxnEntrySizeLimit(), 123);
            session.SetTxnEntrySizeLimit(before);
            Ok(())
        })
        .unwrap();
}

#[test]
fn history_selects_error_before_times() {
    let manager = TaskManager::new();
    let mut cells = vec![
        Cell::Int(5),
        Cell::String("history-task-5".into()),
        Cell::String("ImportInto".into()),
        Cell::String("failed".into()),
        Cell::Int(1),
        Cell::Int(512),
        Cell::Int(8),
        Cell::Time(UNIX_EPOCH),
        Cell::String("".into()),
        Cell::Int(0),
        Cell::Json("{}".into()),
        Cell::String("ks1".into()),
    ];
    cells.extend([
        Cell::Bytes(br#"{"message":"history task failed"}"#.to_vec()),
        Cell::Time(UNIX_EPOCH + std::time::Duration::from_secs(10)),
        Cell::Time(UNIX_EPOCH + std::time::Duration::from_secs(20)),
        Cell::Time(UNIX_EPOCH + std::time::Duration::from_secs(30)),
    ]);
    manager.push_result(vec![chunk::Row::new(cells)]);
    manager.push_result(vec![chunk::Row::new(vec![Cell::Int(5)])]);
    let page = manager.ListHistoryTasks((), 2, 0, "".into()).unwrap();
    assert!(
        manager.calls()[0]
            .sql
            .contains("t.error, t.start_time, t.state_update_time, t.end_time")
    );
    assert_eq!(
        page.Items[0].StartTime,
        UNIX_EPOCH + std::time::Duration::from_secs(10)
    );
    assert_eq!(
        page.Items[0].StateUpdateTime,
        UNIX_EPOCH + std::time::Duration::from_secs(20)
    );
    assert_eq!(
        page.Items[0].EndTime,
        UNIX_EPOCH + std::time::Duration::from_secs(30)
    );
}

#[test]
fn history_error_metadata_handles_null_and_malformed() {
    for (error, category, code) in [
        (Cell::Null, "", ""),
        (Cell::Bytes(b"not-json: secret".to_vec()), "failed", ""),
        (
            Cell::Bytes(br#"{"message":"secret","rfccode":"kv:1062","code":1062}"#.to_vec()),
            "failed",
            "kv:1062",
        ),
    ] {
        let mut row = vec![
            Cell::Int(5),
            Cell::String("history-task-5".into()),
            Cell::String("ImportInto".into()),
            Cell::String("failed".into()),
            Cell::Int(1),
            Cell::Int(512),
            Cell::Int(8),
            Cell::Time(UNIX_EPOCH),
            Cell::String("".into()),
            Cell::Int(0),
            Cell::Json("{}".into()),
            Cell::String("ks1".into()),
        ];
        row.extend([error, Cell::Null, Cell::Null, Cell::Null]);
        let manager = TaskManager::new();
        manager.push_result(vec![chunk::Row::new(row)]);
        manager.push_result(vec![chunk::Row::new(vec![Cell::Int(1)])]);
        let page = manager.ListHistoryTasks((), 2, 0, "".into()).unwrap();
        assert_eq!(page.Items[0].ErrorCategory, category);
        assert_eq!(page.Items[0].ErrorCode, code);
        assert_eq!(page.Items[0].StartTime, UNIX_EPOCH);
        assert_eq!(page.Items[0].EndTime, UNIX_EPOCH);
    }
}

#[test]
fn history_error_classification_follows_replacement() {
    assert_eq!(ClassifyTaskError("failed", None), "");
    for (state, message, expected) in [
        ("failed", "cancelled by user", "failed"),
        ("reverted", "wrapped: cancelled by user", "cancelled"),
        (
            "reverted",
            "ErrEncodeKV Value conversion failed for column x",
            "data-error",
        ),
        (
            "reverted",
            "ErrEncodeKV Check constraint 'c' is violated",
            "data-error",
        ),
        (
            "reverted",
            "ErrEncodeKV Table has no partition for value 1",
            "data-error",
        ),
        (
            "reverted",
            "[executor:8167]Duplicate key conflict found",
            "data-error",
        ),
        (
            "reverted",
            "ErrFoundDataConflictRecords found data conflict records",
            "data-error",
        ),
        (
            "reverted",
            "ErrFoundIndexConflictRecords found index conflict records",
            "data-error",
        ),
        ("reverted", "[kv:1062]Duplicate entry '1'", "data-error"),
        ("reverted", "ErrEncodeKV unrelated", "failed"),
        ("reverted", "Duplicate entry without code", "failed"),
        ("reverted", "arbitrary secret", "failed"),
        ("succeed", "arbitrary secret", ""),
    ] {
        assert_eq!(
            ClassifyTaskError(state, Some(&Error::new(message))),
            expected,
            "{message}"
        );
    }
}

#[test]
fn history_error_metadata_preserves_go_error_code_variants() {
    for (key, state, error, code, category) in [
        (
            "named-failure",
            "failed",
            r#"{"message":"sensitive named failure","rfccode":"DXF:History:Named","code":0}"#,
            "DXF:History:Named",
            "failed",
        ),
        (
            "two-part-code",
            "failed",
            r#"{"message":"sensitive two-part failure","rfccode":"kv:1062","code":0}"#,
            "kv:1062",
            "failed",
        ),
        (
            "code-less-normalized",
            "failed",
            r#"{"message":"sensitive code-less failure","rfccode":"","code":0}"#,
            "",
            "failed",
        ),
        (
            "plain-failure",
            "failed",
            r#"{"message":"sensitive plain failure","rfccode":"","code":0}"#,
            "",
            "failed",
        ),
        (
            "legacy-kv-code",
            "failed",
            r#"{"class":8,"code":1062,"message":"sensitive legacy kv failure","rfccode":""}"#,
            "kv:1062",
            "failed",
        ),
        (
            "numeric-code-only",
            "failed",
            r#"{"class":0,"code":1062,"message":"sensitive legacy numeric failure","rfccode":""}"#,
            "1062",
            "failed",
        ),
        (
            "cancelled",
            "reverted",
            r#"{"message":"cancelled by user","rfccode":"","code":0}"#,
            "",
            "cancelled",
        ),
        (
            "data-error",
            "reverted",
            r#"{"message":"[Lightning:Restore:ErrEncodeKV]Value conversion failed for column 'a'","rfccode":"","code":0}"#,
            "",
            "data-error",
        ),
        (
            "ordinary-revert",
            "reverted",
            r#"{"message":"sensitive ordinary failure","rfccode":"","code":0}"#,
            "",
            "failed",
        ),
    ] {
        let row = chunk::Row::new(vec![
            Cell::Int(42),
            Cell::String(key.into()),
            Cell::String("ImportInto".into()),
            Cell::String(state.into()),
            Cell::Int(1),
            Cell::Int(512),
            Cell::Int(8),
            Cell::Time(UNIX_EPOCH),
            Cell::String("".into()),
            Cell::Int(0),
            Cell::Json("{}".into()),
            Cell::String("ks1".into()),
            Cell::Bytes(error.as_bytes().to_vec()),
            Cell::Null,
            Cell::Null,
            Cell::Null,
        ]);
        let manager = TaskManager::new();
        manager.push_result(vec![row]);
        manager.push_result(vec![chunk::Row::new(vec![Cell::Int(1)])]);
        let page = manager.ListHistoryTasks((), 20, 0, "ks1".into()).unwrap();
        assert_eq!(page.Items.len(), 1);
        assert_eq!(page.Items[0].TaskBase.Key, key);
        assert_eq!(page.Items[0].ErrorCode, code, "{key}");
        assert_eq!(page.Items[0].ErrorCategory, category, "{key}");
    }
}
