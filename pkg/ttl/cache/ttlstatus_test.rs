// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// mysql.tidb_ttl_table_status 行解码与 TableStatusCache 的单元测试。
//
// 覆盖逐列映射、默认 waiting 状态，以及缓存 Update 增删同步。

use std::time::Duration;

use crate::task::Datum;
use crate::ttlstatus::{
    JobStatusRunning, JobStatusWaiting, NewTableStatusCache, RowToTableStatus, StatusSession,
};

/// 构造 17 列状态行，可用 overrides 覆盖指定列。
fn status_row(overrides: &[(usize, Datum)]) -> Vec<Datum> {
    let mut row = vec![Datum::Null; 17];
    row[0] = Datum::Int(0);
    for (index, value) in overrides {
        row[*index] = value.clone();
    }
    row
}

// 对应 Go 用例中逐列检查系统表字段的表驱动测试：验证每一列都会被解码到正确的字段。
#[test]
fn test_row_to_table_status_decodes_every_column() {
    let row = status_row(&[
        (0, Datum::Int(0)),
        (1, Datum::Int(2)),
        (2, Datum::String("test str".into())),
        (3, Datum::String("test job id".into())),
        (4, Datum::Time(1)),
        (5, Datum::Time(2)),
        (6, Datum::Time(3)),
        (7, Datum::String("test summary".into())),
        (8, Datum::String("test current job id".into())),
        (9, Datum::String("test current job owner id".into())),
        (10, Datum::String("addr".into())),
        (11, Datum::Time(4)),
        (12, Datum::Time(5)),
        (13, Datum::Time(6)),
        (14, Datum::String("test state".into())),
        (15, Datum::String("running".into())),
        (16, Datum::Time(7)),
    ]);
    let status = RowToTableStatus(&row).unwrap();
    assert_eq!(status.TableID, 0);
    assert_eq!(status.ParentTableID, 2);
    assert_eq!(status.TableStatistics, "test str");
    assert_eq!(status.LastJobID, "test job id");
    assert_eq!(status.LastJobStartTime, 1);
    assert_eq!(status.LastJobFinishTime, 2);
    assert_eq!(status.LastJobTTLExpire, 3);
    assert_eq!(status.LastJobSummary, "test summary");
    assert_eq!(status.CurrentJobID, "test current job id");
    assert_eq!(status.CurrentJobOwnerID, "test current job owner id");
    assert_eq!(status.CurrentJobOwnerAddr, "addr");
    assert_eq!(status.CurrentJobOwnerHBTime, 4);
    assert_eq!(status.CurrentJobStartTime, 5);
    assert_eq!(status.CurrentJobTTLExpire, 6);
    assert_eq!(status.CurrentJobState, "test state");
    assert_eq!(status.CurrentJobStatus, JobStatusRunning);
    assert_eq!(status.CurrentJobStatusUpdateTime, 7);
}

// 对应 Go：非 NULL 空串回退 waiting；NULL 保持 JobStatus 的字符串零值；未知值原样保留。
#[test]
fn test_row_to_table_status_preserves_job_status_string_semantics() {
    let null_status = RowToTableStatus(&status_row(&[])).unwrap();
    assert_eq!(null_status.CurrentJobStatus, "");

    let empty_status =
        RowToTableStatus(&status_row(&[(15, Datum::String(String::new()))])).unwrap();
    assert_eq!(empty_status.CurrentJobStatus, JobStatusWaiting);

    let unknown_status =
        RowToTableStatus(&status_row(&[(15, Datum::String("test status".into()))])).unwrap();
    assert_eq!(unknown_status.CurrentJobStatus, "test status");
}

// 列数不足 17 时拒绝解码。
#[test]
fn test_row_to_table_status_rejects_short_row() {
    let row = status_row(&[])[..10].to_vec();
    assert!(RowToTableStatus(&row).is_err());
}

/// 返回固定行集的 StatusSession 桩。
struct FakeStatusSession {
    rows: Vec<Vec<Datum>>,
}
impl StatusSession for FakeStatusSession {
    fn execute_sql(&self, _sql: &str) -> Result<Vec<Vec<Datum>>, String> {
        Ok(self.rows.clone())
    }
}

// 对应 Go TestTTLStatusCache：新缓存立即需要刷新；Update 后按 table_id 建立 map；
// 系统表清空后 Update 会把缓存也清空。
#[test]
fn test_table_status_cache_update_syncs_new_and_removed_entries() {
    let mut cache = NewTableStatusCache(Duration::from_secs(3600));
    assert!(cache.ShouldUpdate());

    let mut session = FakeStatusSession {
        rows: vec![status_row(&[(0, Datum::Int(1)), (1, Datum::Int(2))])],
    };
    cache.Update(&session).unwrap();
    assert!(!cache.ShouldUpdate());
    assert_eq!(cache.Tables.len(), 1);
    assert_eq!(cache.Tables[&1].ParentTableID, 2);

    session.rows.clear();
    cache.Update(&session).unwrap();
    assert_eq!(cache.Tables.len(), 0);
}
