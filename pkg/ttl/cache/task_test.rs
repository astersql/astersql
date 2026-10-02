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

// mysql.tidb_ttl_task 行编解码与 SQL 构造的单元测试。
//
// 验证 Insert/Select/Peek SQL 形态，以及 RowToTTLTask 对状态与 JSON state 的解码。

use crate::task::{
    Datum, InsertIntoTTLTask, PeekWaitingTTLTask, RowToTTLTask, SelectFromTTLTaskWithID,
    SelectFromTTLTaskWithJobID, TTLTaskState, TaskStatus,
};

/// 按系统表列顺序构造一行 TTL task Datum。
fn task_row(status: &str, state: Option<&str>) -> Vec<Datum> {
    vec![
        Datum::String("test-job".into()),
        Datum::Int(1),
        Datum::Int(1),
        Datum::Bytes(Vec::new()),
        Datum::Bytes(Vec::new()),
        Datum::Time(100),
        Datum::String("owner".into()),
        Datum::String("addr".into()),
        Datum::Time(90),
        Datum::String(status.into()),
        Datum::Time(80),
        match state {
            Some(value) => Datum::String(value.into()),
            None => Datum::Null,
        },
        Datum::Time(70),
    ]
}

// 对应 Go TestInsertIntoTTLTask + TestRowToTTLTask：本包不启动 Domain 或 TTL job
// manager，因此没有后台 task GC 与测试行竞争。直接构造 InsertIntoTTLTask
// 产出的参数并解码回 TTLTask，验证编码/解码是可逆的。
#[test]
fn insert_into_ttl_task_round_trip_without_background_gc() {
    let start = vec![Datum::Int(1)];
    let end = vec![Datum::Int(2)];
    let (sql, args) = InsertIntoTTLTask("test-job", 1, 1, &start, &end, 100, 100).unwrap();
    assert_eq!(sql, crate::task::insertIntoTTLTask);
    assert_eq!(args[0], Datum::String("test-job".into()));
    assert_eq!(args[1], Datum::Int(1));
    assert_eq!(args[2], Datum::Int(1));
    let Datum::Bytes(encoded_start) = &args[3] else {
        panic!("expected encoded scan_range_start bytes")
    };
    let Datum::Bytes(encoded_end) = &args[4] else {
        panic!("expected encoded scan_range_end bytes")
    };
    assert_eq!(crate::task::DecodeDatums(encoded_start).unwrap(), start);
    assert_eq!(crate::task::DecodeDatums(encoded_end).unwrap(), end);
}

// Go codec.EncodeKey 的 int flag 为 3，后接翻转符号位的大端整数。
#[test]
fn test_encode_datums_matches_go_key_codec() {
    assert_eq!(
        crate::task::EncodeDatums(&[Datum::Int(1)]).unwrap(),
        vec![3, 0x80, 0, 0, 0, 0, 0, 0, 1]
    );
    assert_eq!(
        crate::task::EncodeDatums(&[Datum::Bytes(b"abc".to_vec())]).unwrap(),
        vec![1, b'a', b'b', b'c', 0, 0, 0, 0, 0, 250]
    );
}

// 空扫描范围编码为空字节，对应 Go 中 NULL/空 codec 键。
#[test]
fn test_insert_into_ttl_task_with_null_range() {
    let (_, args) = InsertIntoTTLTask("test-job", 1, 1, &[], &[], 100, 100).unwrap();
    let Datum::Bytes(encoded_start) = &args[3] else {
        panic!("expected encoded scan_range_start bytes")
    };
    assert!(encoded_start.is_empty());
}

// 空 status 回退为 Waiting；无 state 列时 State 为 None。
#[test]
fn test_row_to_ttl_task_null_status_defaults_to_waiting() {
    let row = task_row("", None);
    let task = RowToTTLTask(&row).unwrap();
    assert_eq!(task.JobID, "test-job");
    assert_eq!(task.TableID, 1);
    assert_eq!(task.ScanID, 1);
    assert!(task.ScanRangeStart.is_empty());
    assert!(task.ScanRangeEnd.is_empty());
    assert_eq!(task.ExpireTime, 100);
    assert_eq!(task.CreatedTime, 70);
    assert_eq!(task.Status, TaskStatus::Waiting);
    assert!(task.State.is_none());
}

// running + JSON state 应解码出进度计数器字段。
#[test]
fn test_row_to_ttl_task_running_with_state() {
    let row = task_row(
        "running",
        Some(r#"{"total_rows":10,"success_rows":9,"error_rows":1}"#),
    );
    let task = RowToTTLTask(&row).unwrap();
    assert_eq!(task.Status, TaskStatus::Running);
    let state = task.State.expect("state should be decoded");
    assert_eq!(
        state,
        TTLTaskState {
            TotalRows: 10,
            SuccessRows: 9,
            ErrorRows: 1,
            ScanTaskErr: String::new(),
            PreviousOwner: String::new(),
        }
    );
}

// Go 的 TaskStatus 是 string 别名：未知值必须原样保留，NULL 则保持字符串零值。
#[test]
fn test_row_to_ttl_task_preserves_status_string() {
    let mut row = task_row("paused", None);
    let task = RowToTTLTask(&row).unwrap();
    assert_eq!(task.Status.as_str(), "paused");

    row[9] = Datum::Null;
    let task = RowToTTLTask(&row).unwrap();
    assert_eq!(task.Status.as_str(), "");
}

// encoding/json 会解析转义并拒绝畸形 JSON；Rust 行映射必须传播同样的错误。
#[test]
fn test_row_to_ttl_task_state_json_semantics() {
    let row = task_row(
        "running",
        Some(r#"{"scan_task_err":"line\n\"quoted\"","prev_owner":"node\u0031"}"#),
    );
    let state = RowToTTLTask(&row).unwrap().State.unwrap();
    assert_eq!(state.ScanTaskErr, "line\n\"quoted\"");
    assert_eq!(state.PreviousOwner, "node1");

    let row = task_row("running", Some("{not-json}"));
    assert!(RowToTTLTask(&row).is_err());
}

// 列数不足 13 时拒绝解码。
#[test]
fn test_row_to_ttl_task_rejects_short_row() {
    let row = task_row("waiting", None)[..5].to_vec();
    assert!(RowToTTLTask(&row).is_err());
}

// 校验按 job/scan/peek 三种查询的 WHERE 子句与参数绑定。
#[test]
fn test_select_from_ttl_task_sql_shape() {
    let (sql, args) = SelectFromTTLTaskWithJobID("job-1");
    assert!(sql.contains("WHERE job_id = %?"));
    assert_eq!(args, vec![Datum::String("job-1".into())]);

    let (sql, args) = SelectFromTTLTaskWithID("job-1", 3);
    assert!(sql.contains("WHERE job_id = %? AND scan_id = %?"));
    assert_eq!(args, vec![Datum::String("job-1".into()), Datum::Int(3)]);

    let (sql, args) = PeekWaitingTTLTask(500);
    assert!(sql.contains("status = 'waiting'"));
    assert_eq!(args, vec![Datum::Time(500)]);
}
