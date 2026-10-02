// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// storage 转换器、历史搬迁、节点查询与子任务状态的迁移回归测试。
//
// 用内存假 Session/SQLExecutor 对照 Go 行为：列映射、错误回退、
// keyset 分页 SQL、TransferTasks2History 顺序，以及 CAS/AffectedRows 语义。

use astersql_dxf_framework_storage::*;
use std::time::{Duration, UNIX_EPOCH};

/// 扩展 assert_eq：支持 `assert_eq!(expr, Err(x))` 形式的错误断言。
macro_rules! assert_eq {
    ($left:expr, Err($right:expr) $(,)?) => {{
        match $left {
            Err(actual) if actual == $right => {}
            actual => panic!("assertion failed: left={actual:?}, right=Err({:?})", $right),
        }
    }};
    ($left:expr, $right:expr $(,)?) => {
        ::std::assert_eq!($left, $right)
    };
}

/// 构造完整 task 行（18 列），用于 Row2Task 列映射测试。
fn task_row(extra: &str, error: Cell, modify: &str) -> chunk::Row {
    chunk::Row::new(vec![
        Cell::Int(42),
        Cell::String("task-key".into()),
        Cell::String("Example".into()),
        Cell::String("running".into()),
        Cell::Int(2),
        Cell::Int(100),
        Cell::Int(8),
        Cell::Time(UNIX_EPOCH + Duration::from_secs(10)),
        Cell::String("background".into()),
        Cell::Int(3),
        Cell::Json(extra.into()),
        Cell::String("ks1".into()),
        Cell::Time(UNIX_EPOCH + Duration::from_secs(20)),
        Cell::Time(UNIX_EPOCH + Duration::from_secs(30)),
        Cell::Bytes(vec![1, 2]),
        Cell::String("scheduler-1".into()),
        error,
        Cell::Json(modify.into()),
    ])
}

/// 构造带固定字段的 TaskBase 测试夹具。
fn task_base(id: i64, state: proto::TaskState) -> proto::TaskBase {
    proto::TaskBase {
        ID: id,
        Key: format!("task-{id}"),
        Type: proto::TaskTypeExample,
        State: state,
        Step: proto::StepOne,
        Priority: 100,
        RequiredSlots: 8,
        TargetScope: "background".into(),
        CreateTime: UNIX_EPOCH + Duration::from_secs(10),
        MaxNodeCount: 2,
        ExtraParams: proto::ExtraParams::default(),
        Keyspace: "ks1".into(),
    }
}

/// 在 TaskBase 之上构造 Task（含默认 ModifyParam）。
fn task(id: i64, state: proto::TaskState) -> proto::Task {
    proto::Task {
        TaskBase: task_base(id, state),
        SchedulerID: String::new(),
        StartTime: UNIX_EPOCH,
        StateUpdateTime: UNIX_EPOCH,
        Meta: vec![id as u8],
        Error: None,
        ModifyParam: proto::ModifyParam {
            PrevState: state,
            Modifications: Vec::new(),
        },
    }
}

/// 构造历史任务摘要行（含 end_time 列）。
fn history_row(id: i64) -> chunk::Row {
    chunk::Row::new(vec![
        Cell::Int(id),
        Cell::String(format!("task-{id}")),
        Cell::String("Example".into()),
        Cell::String("succeed".into()),
        Cell::Int(-2),
        Cell::Int(100),
        Cell::Int(8),
        Cell::Time(UNIX_EPOCH + Duration::from_secs(10)),
        Cell::String("background".into()),
        Cell::Int(2),
        Cell::Json("{}".into()),
        Cell::String("ks1".into()),
        Cell::Null,
        Cell::Time(UNIX_EPOCH + Duration::from_secs(20)),
        Cell::Time(UNIX_EPOCH + Duration::from_secs(30)),
        Cell::Time(UNIX_EPOCH + Duration::from_secs(40)),
    ])
}

#[test]
/// 验证 Row2Task：extra_params/modify 解析，以及非 JSON error 回退为原文。
fn converter_matches_go_column_mapping_and_error_fallback() {
    let task = Row2Task(task_row(
        r#"{"manual_recovery":true,"max_runtime_slots":4,"target_steps":[2]}"#,
        Cell::Bytes(b"not-json".to_vec()),
        r#"{"prev_state":"running","modifications":[{"type":"modify_concurrency","to":6}]}"#,
    ));
    assert_eq!(task.ID, 42);
    assert_eq!(task.Type, "Example");
    assert_eq!(task.State, "running");
    assert_eq!(task.RequiredSlots, 8);
    assert!(task.ExtraParams.ManualRecovery);
    assert_eq!(task.ExtraParams.MaxRuntimeSlots, 4);
    assert_eq!(task.Error.as_deref(), Some("not-json"));
    assert_eq!(task.ModifyParam.PrevState, "running");
    assert_eq!(task.ModifyParam.Modifications.len(), 1);
    assert_eq!(task.ModifyParam.Modifications[0].To, 6);
}

#[test]
/// 验证非法 task_id 归零、Int2Type、bigint 秒级时间与 summary 透传。
fn subtask_converter_preserves_go_zero_and_unix_time_semantics() {
    let row = chunk::Row::new(vec![
        Cell::Int(7),
        Cell::Int(1),
        Cell::String("bad-id".into()),
        Cell::Int(2),
        Cell::String("node-1".into()),
        Cell::String("pending".into()),
        Cell::Int(4),
        Cell::Time(UNIX_EPOCH + Duration::from_secs(5)),
        Cell::Null,
        Cell::Int(11),
        Cell::Int(12),
        Cell::Bytes(vec![9]),
        Cell::Json(r#"{"rows":1}"#.into()),
    ]);
    let subtask = Row2SubTask(row);
    assert_eq!(subtask.TaskID, 0);
    assert_eq!(subtask.Type, "ImportInto");
    assert_eq!(subtask.Ordinal, 0);
    assert_eq!(subtask.StartTime, UNIX_EPOCH + Duration::from_secs(11));
    assert_eq!(subtask.UpdateTime, UNIX_EPOCH + Duration::from_secs(12));
    assert_eq!(subtask.Summary, r#"{"rows":1}"#);
}

#[test]
/// 验证分页边界、节点 CPU，以及 Cancel/Pause 生成的状态 SQL。
fn history_validation_nodes_and_state_sql_match_go() {
    assert!(ValidateHistoryTaskPageSize(1).is_ok());
    assert!(ValidateHistoryTaskPageSize(200).is_ok());
    assert_eq!(
        ValidateHistoryTaskPageSize(0).unwrap_err().to_string(),
        "page size should be within [1, 200]"
    );

    SetNodeResource(proto::NewNodeResource(12, 32, 64));
    assert_eq!(GetDXFCPUCount(), 12);

    let manager = TaskManager::new();
    manager.CancelTask((), 9).unwrap();
    manager.PauseSubtasks((), "node-1".into(), 9).unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].sql.contains("state in (%?, %?, %?)"));
    assert_eq!(calls[0].args[1], Value::Int(9));
    assert!(calls[1].sql.contains("state in (\"running\", \"pending\")"));
}

#[test]
/// 验证 StartSubtask：AffectedRows=0 映射 ErrSubtaskNotFound。
fn start_subtask_checks_affected_rows_like_go() {
    let manager = TaskManager::new();
    manager.set_affected_rows(0);
    assert_eq!(
        manager.StartSubtask((), 10, "old-node".into()),
        Err(ErrSubtaskNotFound)
    );
    manager.set_affected_rows(1);
    assert!(manager.StartSubtask((), 10, "node-1".into()).is_ok());
}

#[test]
/// 验证 ListHistoryTasks 的 keyset 条件、HasMore/token 与 count 查询。
fn history_keyset_pagination_and_count_follow_go_queries() {
    let manager = TaskManager::new();
    manager.push_result(vec![history_row(9), history_row(8), history_row(7)]);
    manager.push_result(vec![chunk::Row::new(vec![Cell::Int(5)])]);
    let page = manager.ListHistoryTasks((), 2, 10, "ks1".into()).unwrap();
    assert_eq!(page.Items.len(), 2);
    assert_eq!(page.Items[0].TaskBase.ID, 9);
    assert_eq!(page.Items[1].TaskBase.ID, 8);
    assert!(page.HasMore);
    assert_eq!(page.NextPageToken, 8);
    assert_eq!(page.ApproxTotalCount, 5);
    assert_eq!(page.Items[0].EndTime, UNIX_EPOCH + Duration::from_secs(40));

    let calls = manager.calls();
    assert_eq!(
        calls[0].args,
        vec![Value::String("ks1".into()), Value::Int(10), Value::Int(3)]
    );
    assert!(calls[0].sql.contains("t.keyspace = %? and t.id < %?"));
    assert_eq!(calls[1].args, vec![Value::String("ks1".into())]);
    assert!(calls[1].sql.contains("where keyspace = %?"));
}

#[test]
/// 验证 TransferTasks2History：先 update meta，再 insert/delete，再搬子任务。
fn history_transfer_keeps_go_update_insert_delete_and_subtask_order() {
    let manager = TaskManager::new();
    manager
        .TransferTasks2History(
            (),
            vec![
                task(3, proto::TaskStateSucceed),
                task(5, proto::TaskStateFailed),
            ],
        )
        .unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 8);
    assert!(
        calls[0]
            .sql
            .contains("update mysql.tidb_global_task set meta")
    );
    assert!(
        calls[1]
            .sql
            .contains("update mysql.tidb_global_task set meta")
    );
    assert!(calls[2].sql.contains("where id in(3, 5)"));
    assert!(
        calls[3]
            .sql
            .contains("delete from mysql.tidb_global_task where id in(3, 5)")
    );
    assert!(
        calls[4]
            .sql
            .contains("insert into mysql.tidb_background_subtask_history")
    );
    assert!(
        calls[5]
            .sql
            .contains("delete from mysql.tidb_background_subtask")
    );
    assert!(
        calls[6]
            .sql
            .contains("insert into mysql.tidb_background_subtask_history")
    );
    assert!(
        calls[7]
            .sql
            .contains("delete from mysql.tidb_background_subtask")
    );
}

#[test]
/// 验证 GetAllNodes 排序、GetUsedSlotsOnNodes 聚合与按 role 取 CPU。
fn node_queries_preserve_order_role_filter_and_slot_aggregation() {
    let manager = TaskManager::new();
    manager.push_result(vec![
        chunk::Row::new(vec![
            Cell::String("n1".into()),
            Cell::String("".into()),
            Cell::Int(4),
        ]),
        chunk::Row::new(vec![
            Cell::String("n2".into()),
            Cell::String("background".into()),
            Cell::Int(16),
        ]),
    ]);
    let nodes = manager.GetAllNodes(()).unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].ID, "n1");

    manager.push_result(vec![
        chunk::Row::new(vec![Cell::String("n1".into()), Cell::Decimal(7)]),
        chunk::Row::new(vec![Cell::String("n2".into()), Cell::Decimal(11)]),
    ]);
    let slots = manager.GetUsedSlotsOnNodes(()).unwrap();
    assert_eq!(slots.get("n1"), Some(&7));
    assert_eq!(slots.get("n2"), Some(&11));

    manager.push_result(vec![
        chunk::Row::new(vec![
            Cell::String("n1".into()),
            Cell::String("".into()),
            Cell::Int(4),
        ]),
        chunk::Row::new(vec![
            Cell::String("n2".into()),
            Cell::String("background".into()),
            Cell::Int(16),
        ]),
    ]);
    assert_eq!(
        manager
            .GetCPUCountOfNodeByRole((), "background".into())
            .unwrap(),
        16
    );
}

#[test]
/// 验证 PauseTaskOnError：任务 CAS 失败返回 ErrTaskChanged，成功则清 end_time。
fn pause_on_error_is_transactional_and_checks_task_cas() {
    let manager = TaskManager::new();
    manager.set_affected_rows(0);
    assert_eq!(
        manager.PauseTaskOnError(
            (),
            4,
            proto::TaskStateRunning,
            proto::StepOne,
            Error::new("boom")
        ),
        Err(ErrTaskChanged),
    );
    assert_eq!(manager.calls().len(), 1);

    let manager = TaskManager::new();
    manager.set_affected_rows(1);
    manager
        .PauseTaskOnError(
            (),
            4,
            proto::TaskStateRunning,
            proto::StepOne,
            Error::new("boom"),
        )
        .unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].sql.contains("end_time = null"));
    assert_eq!(calls[1].args[1], Value::Int(4));
}

#[test]
/// 验证 ModifyTaskByID 状态门禁与 ModifiedTask 更新活跃子任务。
fn modify_task_validates_previous_state_and_updates_active_subtasks() {
    let manager = TaskManager::new();
    let invalid = proto::ModifyParam {
        PrevState: proto::TaskStateFailed,
        Modifications: Vec::new(),
    };
    assert_eq!(
        manager.ModifyTaskByID((), 1, invalid),
        Err(ErrTaskStateNotAllow)
    );
    assert!(manager.calls().is_empty());

    manager.set_task_state(proto::TaskStateRunning);
    manager.set_affected_rows(1);
    let param = proto::ModifyParam {
        PrevState: proto::TaskStateRunning,
        Modifications: vec![proto::Modification {
            Type: proto::ModifyRequiredSlots,
            To: 6,
        }],
    };
    manager.ModifyTaskByID((), 1, param).unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].sql.contains("modify_params = %?"));
    match &calls[0].args[1] {
        Value::Bytes(bytes) => {
            assert!(String::from_utf8_lossy(bytes).contains("modify_concurrency"))
        }
        value => panic!("expected JSON bytes, got {value:?}"),
    }

    let mut changed = task(1, proto::TaskStateModifying);
    changed.RequiredSlots = 6;
    changed.MaxNodeCount = 3;
    changed.ModifyParam.PrevState = proto::TaskStateRunning;
    manager.ModifiedTask((), changed).unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 3);
    assert!(calls[2].sql.contains("state in (%?, %?, %?)"));
    assert_eq!(calls[2].args[0], Value::Int(6));
}
