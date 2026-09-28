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

// `GetFromHistory` 历史聚合测试与 Go 草稿对照。
//
// 草稿描述基于真实 history 表的聚合期望；可执行用例用规范 store
// 注入任务行与子任务行，校验文件大小、KV 体积、行长与分步骤耗时。

const _GO_HISTORY_TEST_DRAFT: &str = r###"
// 这段逻辑只描述 history 查询测试，不连接真实数据库，也不执行后台任务迁移。

// test_get_from_history 对应 Go 的 TestGetFromHistory。
// 它构造一条 IMPORT INTO task、多个 subtask，再转入 history 表验证聚合出的展示字段。
#[test]
fn test_get_from_history() {
    let (_store, tm, ctx) = testutil::InitTableTest();
    assert_ok!(tm.InitMeta(ctx, ":4000", ""));

    const KEYSPACE: &str = "ks1";
    const JOB_ID: i64 = 9527;
    let task_meta = br#"{
        "Plan": {
            "DistSQLScanConcurrency": 16,
            "DesiredTableInfo": {
                "index_info": [{"id": 1}, {"id": 2}],
                "cols": [{"id": 1}, {"id": 2}, {"id": 3}]
            },
            "TotalFileSize": 2147483648
        },
        "Summary": {
            "row-count": 1024
        }
    }"#.to_vec();

    let task_id = tm.CreateTask(
        ctx,
        taskkey::ForJobInKeyspace(KEYSPACE, JOB_ID),
        proto::ImportInto,
        KEYSPACE,
        8,
        "",
        4,
        proto::ExtraParams::default(),
        task_meta,
    ).expect("Go require.NoError(t, err)");

    let encode_id = testutil::InsertSubtask(
        &tm, task_id, proto::ImportStepEncodeAndSort, "tidb-1",
        br#"{"kv-group":"data"}"#, proto::SubtaskStateSucceed, proto::ImportInto, 8,
    );
    let data_id = testutil::InsertSubtask(
        &tm, task_id, proto::ImportStepWriteAndIngest, "tidb-1",
        br#"{"kv-group":"data"}"#, proto::SubtaskStateSucceed, proto::ImportInto, 8,
    );
    let index_id = testutil::InsertSubtask(
        &tm, task_id, proto::ImportStepWriteAndIngest, "tidb-1",
        br#"{"kv-group":"index-1"}"#, proto::SubtaskStateSucceed, proto::ImportInto, 8,
    );
    let invalid_duration_id = testutil::InsertSubtask(
        &tm, task_id, proto::ImportStepPostProcess, "tidb-1",
        br#"{"kv-group":"data"}"#, proto::SubtaskStateSucceed, proto::ImportInto, 8,
    );

    let update_subtask = |id: i64, start_time: i64, end_time: i64, summary: &str, meta: &str| {
        // Go 直接更新 mysql.tidb_background_subtask 的时间、summary、meta；这里保留 SQL 与参数顺序。
        assert_ok!(tm.ExecuteSQLWithNewSession(ctx, r#"
            update mysql.tidb_background_subtask
            set start_time = %?, state_update_time = %?, summary = %?, meta = %?
            where id = %?"#,
            start_time, end_time, summary, meta.as_bytes(), id,
        ));
    };
    update_subtask(encode_id, 100, 700, r#"{"bytes": 1073741824}"#, r#"{"kv-group":"data"}"#);
    update_subtask(data_id, 700, 2500, r#"{"bytes": 1073741824}"#, r#"{"kv-group":"data"}"#);
    update_subtask(index_id, 900, 2100, r#"{"bytes": 536870912}"#, r#"{"kv-group":"index-1"}"#);
    update_subtask(invalid_duration_id, 0, 0, r#"{"bytes": 0}"#, r#"{"kv-group":"data"}"#);

    let task = tm.GetTaskByID(ctx, task_id).expect("Go require.NoError(t, err)");
    assert_ok!(tm.TransferTasks2History(ctx, vec![task]));

    let info = jobhistory::GetFromHistory(ctx, &tm, KEYSPACE, JOB_ID).expect("Go require.NoError(t, err)");
    assert_eq!(JOB_ID, info.JobID);
    assert_eq!(KEYSPACE, info.Keyspace);
    assert_eq!(task_id, info.TaskID);
    assert_eq!(proto::TaskStatePending.to_string(), info.State);
    assert_eq!(8, info.Concurrency);
    assert_eq!(4, info.MaxNodeCount);
    assert_eq!(16, info.DistSQLScanConcurrency);
    assert_eq!(2, info.IndexCount);
    assert_eq!(3, info.ColumnCount);
    assert_eq!("2GiB", info.FileSize);
    assert_eq!("1GiB", info.DataKVSize);
    assert_eq!("512MiB", info.IndexKVSize);
    assert!(!info.PerCoreSpeed.is_empty());
    assert!(!info.OverallSpeed.is_empty());
    assert_eq!(1024, info.RowCount);
    assert_eq!(2097152, info.RowLength);
    assert_eq!("40m0s", info.Duration.Total);
    assert_eq!("10m0s", info.Duration.Encode);
    assert_eq!("30m0s", info.Duration.Ingest);
    assert!(info.Duration.MergeSort.is_empty());
    assert!(info.Duration.CollectConflicts.is_empty());
    assert!(info.Duration.ResolveConflicts.is_empty());
    assert!(info.Duration.PostProcess.is_empty());

    let err = jobhistory::GetFromHistory(ctx, &tm, KEYSPACE, JOB_ID + 1).unwrap_err();
    assert_contains!(err, "not found in history");
}
"###;

use crate::{GetFromHistory, Row, Value, proto, storage};

#[test]
/// 经规范 TaskManager 注入两轮查询结果，验证历史字段聚合。
fn history_aggregates_task_and_subtask_rows_via_canonical_store() {
    let manager = storage::TaskManager::new();
    manager.push_result(vec![Row::new(vec![
        Value::Int(42),
        Value::String(proto::TaskStatePending.to_string()),
        Value::Int(8),
        Value::Int(4),
        Value::Int(16),
        Value::Int(2),
        Value::Int(3),
        Value::Int(2_147_483_648),
        Value::Int(1024),
    ])]);
    manager.push_result(vec![
        Row::new(vec![
            Value::Int(proto::ImportStepEncodeAndSort),
            Value::String("data".to_string()),
            Value::Int(1_073_741_824),
            Value::Int(100),
            Value::Int(700),
        ]),
        Row::new(vec![
            Value::Int(proto::ImportStepWriteAndIngest),
            Value::String("data".to_string()),
            Value::Int(1_073_741_824),
            Value::Int(700),
            Value::Int(2500),
        ]),
        Row::new(vec![
            Value::Int(proto::ImportStepWriteAndIngest),
            Value::String("index-1".to_string()),
            Value::Int(536_870_912),
            Value::Int(900),
            Value::Int(2100),
        ]),
        Row::new(vec![
            Value::Int(proto::ImportStepPostProcess),
            Value::String("data".to_string()),
            Value::Int(0),
            Value::Null,
            Value::Null,
        ]),
    ]);

    let info = GetFromHistory((), &manager, "ks1", 9527).unwrap();
    assert_eq!(info.JobID, 9527);
    assert_eq!(info.Keyspace, "ks1");
    assert_eq!(info.TaskID, 42);
    assert_eq!(info.State, proto::TaskStatePending.to_string());
    assert_eq!(info.Concurrency, 8);
    assert_eq!(info.MaxNodeCount, 4);
    assert_eq!(info.DistSQLScanConcurrency, 16);
    assert_eq!(info.IndexCount, 2);
    assert_eq!(info.ColumnCount, 3);
    assert_eq!(info.FileSize, "2GiB");
    assert_eq!(info.DataKVSize, "1GiB");
    assert_eq!(info.IndexKVSize, "512MiB");
    assert_eq!(info.PerCoreSpeed, "96MiB/core/hour");
    assert_eq!(info.OverallSpeed, "3GiB/hour");
    assert_eq!(info.RowCount, 1024);
    assert_eq!(info.RowLength, 2_097_152);
    assert_eq!(info.Duration.Total, "40m0s");
    assert_eq!(info.Duration.Encode, "10m0s");
    assert_eq!(info.Duration.Ingest, "30m0s");
    assert!(info.Duration.MergeSort.is_empty());
    assert!(info.Duration.CollectConflicts.is_empty());
    assert!(info.Duration.ResolveConflicts.is_empty());
    assert!(info.Duration.PostProcess.is_empty());

    let missing = storage::TaskManager::new();
    missing.push_result(Vec::new());
    let error = GetFromHistory((), &missing, "ks1", 9528).unwrap_err();
    assert_eq!(error, storage::ErrTaskNotFound);
    assert!(
        error
            .to_string()
            .contains("import-into job 9528 in keyspace ks1 not found in history")
    );
}
