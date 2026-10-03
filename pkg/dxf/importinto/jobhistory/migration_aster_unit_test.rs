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

// jobhistory 迁移对齐单测：校验 GetFromHistory 聚合、步骤时长合并与格式化函数与 Go 一致。

use astersql_dxf_importinto_jobhistory::{
    GetFromHistory, Row, Value, formatBytes, formatBytesPerCoreHour, formatBytesPerHour,
    formatDuration, proto, storage,
};

/// 由单元格值构造一行查询结果。
fn row(values: Vec<Value>) -> Row {
    Row::new(values)
}

/// 构造一条全局任务历史行：task_id/状态/并发/节点数/扫描并发/索引列数/文件大小/行数。
fn history_task_row() -> Row {
    row(vec![
        Value::Int(42),
        Value::String(proto::TaskStatePending.to_owned()),
        Value::Int(8),
        Value::Int(4),
        Value::Int(16),
        Value::Int(2),
        Value::Int(3),
        Value::Int(2_147_483_648),
        Value::Int(1024),
    ])
}

/// 校验从历史表聚合出的作业信息、吞吐与各步骤时长，并核对 SQL 调用顺序与参数。
#[test]
fn migration_get_from_history_matches_go_aggregation_and_sql_order() {
    let manager = storage::TaskManager::new();
    // 第一批结果：全局任务历史行；第二批：各子任务步骤的起止时间与 KV 大小。
    manager.push_result(vec![history_task_row()]);
    manager.push_result(vec![
        row(vec![
            Value::Int(proto::ImportStepEncodeAndSort),
            Value::String("data".to_owned()),
            Value::Int(1_073_741_824),
            Value::Int(100),
            Value::Int(700),
        ]),
        row(vec![
            Value::Int(proto::ImportStepWriteAndIngest),
            Value::String("data".to_owned()),
            Value::Int(1_073_741_824),
            Value::Int(700),
            Value::Int(2500),
        ]),
        row(vec![
            Value::Int(proto::ImportStepWriteAndIngest),
            Value::String("index-1".to_owned()),
            Value::Int(536_870_912),
            Value::Int(900),
            Value::Int(2100),
        ]),
        row(vec![
            Value::Int(proto::ImportStepPostProcess),
            Value::String("data".to_owned()),
            Value::Int(0),
            Value::Null,
            Value::Null,
        ]),
    ]);

    let info = GetFromHistory((), &manager, "ks1", 9527).expect("history job should exist");

    assert_eq!(9527, info.JobID);
    assert_eq!("ks1", info.Keyspace);
    assert_eq!(42, info.TaskID);
    assert_eq!(proto::TaskStatePending, info.State);
    assert_eq!(8, info.Concurrency);
    assert_eq!(4, info.MaxNodeCount);
    assert_eq!(16, info.DistSQLScanConcurrency);
    assert_eq!(2, info.IndexCount);
    assert_eq!(3, info.ColumnCount);
    assert_eq!("2GiB", info.FileSize);
    assert_eq!("1GiB", info.DataKVSize);
    assert_eq!("512MiB", info.IndexKVSize);
    assert_eq!("96MiB/core/hour", info.PerCoreSpeed);
    assert_eq!("3GiB/hour", info.OverallSpeed);
    assert_eq!(1024, info.RowCount);
    assert_eq!(2_097_152, info.RowLength);
    assert_eq!("40m0s", info.Duration.Total);
    assert_eq!("10m0s", info.Duration.Encode);
    assert_eq!("30m0s", info.Duration.Ingest);
    assert!(info.Duration.MergeSort.is_empty());
    assert!(info.Duration.CollectConflicts.is_empty());
    assert!(info.Duration.ResolveConflicts.is_empty());
    assert!(info.Duration.PostProcess.is_empty());

    // 应先查全局任务历史，再按 task_id 查子任务历史。
    let calls = manager.calls();
    assert_eq!(2, calls.len());
    assert!(calls[0].sql.contains("from mysql.tidb_global_task_history"));
    assert_eq!(
        vec![
            Value::String(
                astersql_dxf_importinto_jobhistory::taskkey::ForJobInKeyspace(
                    "ks1".to_owned(),
                    9527,
                )
            ),
            Value::String(proto::ImportInto.to_owned()),
        ],
        calls[0].args
    );
    assert!(
        calls[1]
            .sql
            .contains("from mysql.tidb_background_subtask_history")
    );
    assert_eq!(vec![Value::String("42".into())], calls[1].args);
}

/// 同一步骤多条子任务应合并时间边界；覆盖 merge/collect/resolve/post-process 各步骤映射。
#[test]
fn migration_combines_repeated_step_bounds_and_maps_every_go_step() {
    let manager = storage::TaskManager::new();
    manager.push_result(vec![history_task_row()]);
    manager.push_result(vec![
        row(vec![
            Value::Int(proto::ImportStepMergeSort),
            Value::Null,
            Value::Null,
            Value::Int(300),
            Value::Int(400),
        ]),
        row(vec![
            Value::Int(proto::ImportStepMergeSort),
            Value::Null,
            Value::Null,
            Value::Int(200),
            Value::Int(500),
        ]),
        row(vec![
            Value::Int(proto::ImportStepCollectConflicts),
            Value::Null,
            Value::Null,
            Value::Int(500),
            Value::Int(560),
        ]),
        row(vec![
            Value::Int(proto::ImportStepConflictResolution),
            Value::Null,
            Value::Null,
            Value::Int(560),
            Value::Int(680),
        ]),
        row(vec![
            Value::Int(proto::ImportStepPostProcess),
            Value::Null,
            Value::Null,
            Value::Int(680),
            Value::Int(710),
        ]),
    ]);

    let info = GetFromHistory((), &manager, "ks1", 9527).expect("history job should exist");
    assert_eq!("8m30s", info.Duration.Total);
    assert_eq!("5m0s", info.Duration.MergeSort);
    assert_eq!("1m0s", info.Duration.CollectConflicts);
    assert_eq!("2m0s", info.Duration.ResolveConflicts);
    assert_eq!("30s", info.Duration.PostProcess);
}

/// 任务缺失应报 not found；可选字段为 Null 时与 Go 零值/空字符串语义一致。
#[test]
fn migration_not_found_and_null_fields_match_go_zero_values() {
    let missing = storage::TaskManager::new();
    missing.push_result(Vec::new());
    let error = GetFromHistory((), &missing, "ks1", 9528).unwrap_err();
    assert_eq!(error, storage::ErrTaskNotFound);
    assert!(error.to_string().contains("task not found"));
    assert!(
        error
            .to_string()
            .contains("import-into job 9528 in keyspace ks1 not found in history")
    );

    // 任务行存在但多数列为 Null，且无子任务行时，数值/字符串应回落为零值。
    let manager = storage::TaskManager::new();
    manager.push_result(vec![row(vec![
        Value::Int(43),
        Value::String("succeed".to_owned()),
        Value::Int(1),
        Value::Int(1),
        Value::Null,
        Value::Null,
        Value::Null,
        Value::Null,
        Value::Null,
    ])]);
    manager.push_result(Vec::new());
    let info = GetFromHistory((), &manager, "ks1", 9529).expect("history job should exist");
    assert_eq!(0, info.DistSQLScanConcurrency);
    assert_eq!(0, info.IndexCount);
    assert_eq!(0, info.ColumnCount);
    assert!(info.FileSize.is_empty());
    assert_eq!(0, info.RowCount);
    assert_eq!(0, info.RowLength);
    assert!(info.Duration.Total.is_empty());
    assert!(info.PerCoreSpeed.is_empty());
    assert!(info.OverallSpeed.is_empty());
}

/// 校验时长/字节/吞吐格式化边界，以及 Info 的 JSON 字段名与 Go 一致。
#[test]
fn migration_formatters_and_json_names_match_go() {
    assert_eq!("0s", formatDuration(0));
    assert_eq!("1h1m1s", formatDuration(3661));
    assert_eq!("", formatDuration(-1));
    assert_eq!("0B", formatBytes(0));
    assert_eq!("1.5KiB", formatBytes(1536));
    assert_eq!("1000KiB", formatBytes(1_024_000));
    assert_eq!("", formatBytes(-1));
    assert_eq!("3GiB/hour", formatBytesPerHour(2_147_483_648, 2400));
    assert_eq!(
        "96MiB/core/hour",
        formatBytesPerCoreHour(2_147_483_648, 2400, 4, 8)
    );
    assert_eq!("", formatBytesPerHour(1, 0));
    assert_eq!("1e-05B/hour", formatBytesPerHour(1, 360_000_000));
    assert_eq!("", formatBytesPerCoreHour(1, 1, 0, 1));

    let manager = storage::TaskManager::new();
    manager.push_result(vec![history_task_row()]);
    manager.push_result(Vec::new());
    let info = GetFromHistory((), &manager, "ks1", 9527).expect("history job should exist");
    let json = serde_json::to_value(info).expect("Info should retain Go JSON field names");
    assert_eq!(Some(&serde_json::json!(9527)), json.get("job_id"));
    assert!(json.get("per_core_speed").is_some());
    assert!(json["duration"].get("merge_sort").is_some());
    assert!(json["duration"].get("collect_conflicts").is_some());
}
