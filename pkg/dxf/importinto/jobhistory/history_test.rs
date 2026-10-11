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

// `GetFromHistory` 历史聚合测试。
// 用规范 store 注入任务行与子任务行，校验文件大小、KV 体积、行长与分步骤耗时。

use crate::{GetFromHistory, Row, Value, proto, storage};

#[test]
/// 经规范 TaskManager 注入两轮查询结果，验证历史字段聚合。
fn history_aggregates_task_and_subtask_rows_via_canonical_store() {
    let manager = storage::TaskManager::new();
    manager.push_result(vec![Row::new(vec![
        Value::Int(9_007_199_254_740_993),
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
    assert_eq!(info.TaskID, 9_007_199_254_740_993);
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

    assert_eq!(
        manager.calls()[1].args,
        vec![Value::String("9007199254740993".into())]
    );

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
