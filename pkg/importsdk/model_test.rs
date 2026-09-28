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

// `JobStatus` 终结态判定的单元测试。
//
// 对照 Go `TestJobStatus` 表驱动用例，验证 `IsFinished`/`IsFailed`/
// `IsCancelled`/`IsCompleted` 与各 status 字符串的组合关系。

use crate::JobStatus;
use chrono::NaiveDateTime;

/// 构造仅填充 Status 字段的最小 `JobStatus`，其余字段取空/零值。
fn status(value: &str) -> JobStatus {
    JobStatus {
        JobID: 0,
        GroupKey: String::new(),
        DataSource: String::new(),
        TargetTable: String::new(),
        TableID: 0,
        Phase: String::new(),
        Status: value.to_owned(),
        SourceFileSize: String::new(),
        ImportedRows: 0,
        ResultMessage: String::new(),
        CreateTime: NaiveDateTime::default(),
        StartTime: NaiveDateTime::default(),
        EndTime: NaiveDateTime::default(),
        CreatedBy: String::new(),
        UpdateTime: NaiveDateTime::default(),
        Step: String::new(),
        ProcessedSize: String::new(),
        TotalSize: String::new(),
        Percent: String::new(),
        Speed: String::new(),
        ETA: String::new(),
    }
}

/// Mirrors Go's `TestJobStatus` table-driven test: every status/derived-boolean
/// combination from the Go table is preserved.
/// 对照 Go 表：逐一断言 finished/failed/cancelled/completed 派生布尔值。
#[test]
fn job_completion_states_match_go_table() {
    for (name, finished, failed, cancelled, completed) in [
        ("finished", true, false, false, true),
        ("failed", false, true, false, true),
        ("cancelled", false, false, true, true),
        ("running", false, false, false, false),
        ("pending", false, false, false, false),
        ("unknown", false, false, false, false),
    ] {
        let job = status(name);
        assert_eq!(finished, job.IsFinished(), "status: {name}");
        assert_eq!(failed, job.IsFailed(), "status: {name}");
        assert_eq!(cancelled, job.IsCancelled(), "status: {name}");
        assert_eq!(completed, job.IsCompleted(), "status: {name}");
    }
}
