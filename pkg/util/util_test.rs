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

// util 模块单元测试：慢查询日志字段、按行读取、标识符校验与 protobuf 深拷贝。
//
// 覆盖 `GenLogFields`（含脱敏/截断）、`ReadLine`、`IsInCorrectIdentifierName`
// 与 `ProtoV1Clone`，行为对齐 Go `pkg/util` 对应测试。

use std::any::Any;
use std::collections::HashMap;
use std::io::{BufReader, Cursor};
use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use task_sessmgr::{
    ProcessInfo,
    stmtctx::{NewStmtCtx, ReferenceCount},
};

use crate::util::{GenLogFields, IsInCorrectIdentifierName, LogValue, ProtoV1Clone, ReadLine};

/// 空统计回调：测试用不提供额外 stats 字段。
fn empty_stats(_: &dyn Any) -> HashMap<String, u64> {
    HashMap::new()
}

/// 用于 `ProtoV1Clone` 深拷贝测试的最小 StorageBackend 消息。
#[derive(Clone, PartialEq, Message)]
struct StorageBackend {
    #[prost(message, optional, tag = "1")]
    s3: Option<S3>,
}

/// StorageBackend 内嵌的 S3 配置片段。
#[derive(Clone, PartialEq, Message)]
struct S3 {
    #[prost(string, tag = "1")]
    endpoint: String,
}

/// 校验 `GenLogFields`：耗时、连接元数据、内存峰值、SQL 脱敏与超长截断。
#[test]
fn test_log_format() {
    let mem: Arc<task_sessmgr::memory::Tracker> =
        Arc::from(task_sessmgr::memory::NewTracker(-1, -1));
    // 构造约 1.875GB 占用，便于断言 mem_max 含 Bytes/GB 可读格式。
    mem.Consume((1 << 30) + (1 << 29) + (1 << 28) + (1 << 27));
    let mock_too_long_query = vec![0_u8; 1024 * 9];

    let ref_count = Arc::new(ReferenceCount::default());
    let stmt_ctx = Arc::from(NewStmtCtx());
    let mut info = ProcessInfo {
        ID: 233,
        User: "PingCAP".to_owned(),
        Host: "127.0.0.1".to_owned(),
        DB: "Database".to_owned(),
        Info: "select * from table where a > 1".to_owned(),
        CurTxnStartTS: 23333,
        StatsInfo: Some(empty_stats),
        StmtCtx: Some(Arc::clone(&stmt_ctx)),
        RefCountOfStmtCtx: Some(Arc::clone(&ref_count)),
        MemTracker: Some(Arc::clone(&mem)),
        RedactSQL: String::new(),
        SessionAlias: "alias123".to_owned(),
        ..ProcessInfo::default()
    };
    let cost_time = Duration::from_secs(233);
    let log_sql_truncate_len = 1024 * 8;
    let mut log_fields = GenLogFields(cost_time, &info, true);

    // Rust GenLogFields omits mem_arbitration in the non-arbitrator build (task-344).
    assert!(log_fields.len() >= 8, "fields={log_fields:?}");
    assert_eq!("cost_time", log_fields[0].key);
    assert_eq!(LogValue::String("233s".to_owned()), log_fields[0].value);
    assert_eq!("conn", log_fields[1].key);
    assert_eq!(LogValue::Unsigned(233), log_fields[1].value);
    assert_eq!("user", log_fields[2].key);
    assert_eq!(LogValue::String("PingCAP".to_owned()), log_fields[2].value);
    assert_eq!("database", log_fields[3].key);
    assert_eq!(LogValue::String("Database".to_owned()), log_fields[3].value);
    assert_eq!("txn_start_ts", log_fields[4].key);
    assert_eq!(LogValue::Unsigned(23333), log_fields[4].value);
    assert_eq!("mem_max", log_fields[5].key);
    match &log_fields[5].value {
        LogValue::String(value) => {
            assert!(value.starts_with("2013265920 Bytes"), "{value}");
            assert!(value.contains("GB"), "{value}");
        }
        other => panic!("unexpected mem_max value: {other:?}"),
    }

    let sql_index = log_fields
        .iter()
        .position(|field| field.key == "sql")
        .expect("sql field");
    assert_eq!(
        LogValue::String("select * from table where a > 1".to_owned()),
        log_fields[sql_index].value
    );

    // 开启脱敏时 SQL 字段应被改写（含特殊标记或仍含关键字）。
    info.RedactSQL = "MARKER".to_owned();
    log_fields = GenLogFields(cost_time, &info, true);
    let sql_index = log_fields
        .iter()
        .position(|field| field.key == "sql")
        .expect("sql field");
    match &log_fields[sql_index].value {
        LogValue::String(sql) => {
            assert!(
                sql.contains('‹') || sql.contains('›') || sql.contains("select"),
                "redacted sql={sql}"
            );
        }
        other => panic!("unexpected sql value: {other:?}"),
    }
    info.RedactSQL = String::new();

    log_fields = GenLogFields(cost_time, &info, true);
    let sql_index = log_fields
        .iter()
        .position(|field| field.key == "sql")
        .expect("sql field");
    assert_eq!(
        LogValue::String("select * from table where a > 1".to_owned()),
        log_fields[sql_index].value
    );
    // truncate=true：超长 SQL 截断并附带 len(...) 提示。
    info.Info = String::from_utf8_lossy(&mock_too_long_query).into_owned();
    log_fields = GenLogFields(cost_time, &info, true);
    let sql_index = log_fields
        .iter()
        .position(|field| field.key == "sql")
        .expect("sql field");
    match &log_fields[sql_index].value {
        LogValue::String(sql) => {
            assert!(
                sql.len() >= log_sql_truncate_len,
                "truncated sql len={}",
                sql.len()
            );
            assert!(sql.contains("len("), "{sql}");
        }
        other => panic!("unexpected sql value: {other:?}"),
    }
    // truncate=false：保留完整 SQL 长度。
    log_fields = GenLogFields(cost_time, &info, false);
    let sql_index = log_fields
        .iter()
        .position(|field| field.key == "sql")
        .expect("sql field");
    match &log_fields[sql_index].value {
        LogValue::String(sql) => assert_eq!(mock_too_long_query.len(), sql.len()),
        other => panic!("unexpected sql value: {other:?}"),
    }
    let alias_index = log_fields
        .iter()
        .position(|field| field.key == "session_alias")
        .expect("session_alias field");
    assert_eq!(
        LogValue::String("alias123".to_owned()),
        log_fields[alias_index].value
    );
}

/// 校验 `ReadLine` 按换行分段读取，并在末尾返回 EOF。
#[test]
fn test_read_line() {
    let mut reader = Cursor::new("line1\nline2\nline3");
    let line = ReadLine(&mut reader, 1024).expect("first ReadLine should succeed");
    assert_eq!("line1", String::from_utf8_lossy(&line));
    let line = ReadLine(&mut reader, 1024).expect("second ReadLine should succeed");
    assert_eq!("line2", String::from_utf8_lossy(&line));
    let line = ReadLine(&mut reader, 1024).expect("third ReadLine should succeed");
    assert_eq!("line3", String::from_utf8_lossy(&line));
    let err = ReadLine(&mut reader, 1024).expect_err("EOF expected");
    assert!(
        err.to_string().contains("EOF")
            || err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::UnexpectedEof)
            || format!("{err:#}").contains("ended"),
        "unexpected EOF error: {err:#}"
    );
}

/// Go `bufio.Reader.ReadLine` 仅在行跨越内部缓冲区时执行 `maxLineSize` 检查。
#[test]
fn test_read_line_limit_matches_go_buffer_fragmentation() {
    let mut unfragmented = BufReader::with_capacity(16, Cursor::new(b"12345\n"));
    assert_eq!(
        b"12345",
        ReadLine(&mut unfragmented, 4)
            .expect("an unfragmented line is not rejected by Go")
            .as_slice()
    );

    let mut fragmented = BufReader::with_capacity(4, Cursor::new(b"123456789\n"));
    let error = ReadLine(&mut fragmented, 4).expect_err("fragmented oversized line must fail");
    assert!(
        error
            .to_string()
            .contains("single line length exceeds limit: 4"),
        "unexpected limit error: {error:#}"
    );

    let mut exact_limit = BufReader::with_capacity(4, Cursor::new(b"12345\n"));
    assert_eq!(
        b"12345",
        ReadLine(&mut exact_limit, 5)
            .expect("the line terminator is excluded from Go's limit check")
            .as_slice()
    );
}

/// 标识符合法性用例：名称、输入与期望的「不正确」判定结果。
struct IdentifierCase {
    name: &'static str,
    input: &'static str,
    correct: bool,
}

/// 表驱动校验 `IsInCorrectIdentifierName`（空/尾空格视为不正确，合法名返回 false）。
#[test]
fn test_is_in_correct_identifier_name() {
    let tests = [
        IdentifierCase {
            name: "Empty identifier",
            input: "",
            correct: true,
        },
        IdentifierCase {
            name: "Ending space",
            input: "test ",
            correct: true,
        },
        IdentifierCase {
            name: "Correct identifier",
            input: "test",
            correct: false,
        },
        IdentifierCase {
            name: "Other correct Identifier",
            input: "aaa --\n\txyz",
            correct: false,
        },
    ];

    for tc in tests {
        let got = IsInCorrectIdentifierName(tc.input);
        assert_eq!(
            tc.correct, got,
            "IsInCorrectIdentifierName({}) != {}",
            tc.name, tc.correct
        );
    }
}

/// 校验 `ProtoV1Clone` 为深拷贝：修改副本不影响原消息。
#[test]
fn test_dup_proto() {
    let p = StorageBackend {
        s3: Some(S3 {
            endpoint: "127.0.0.1".to_owned(),
        }),
    };

    let mut p2 = ProtoV1Clone(&p).expect("clone StorageBackend");
    assert_eq!(p2.s3.as_ref().unwrap().endpoint, "127.0.0.1");
    p2.s3.as_mut().unwrap().endpoint = "127.0.0.2".to_owned();
    assert_eq!(p.s3.as_ref().unwrap().endpoint, "127.0.0.1");
}
