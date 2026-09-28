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

// stmtsummary v2 列工厂与辅助函数的共享单元测试。
//
// 覆盖：`avgInt`/`formatBackoffTypes` 等列取值辅助、JSON 日志扁平/淘汰标记、
// 内存读取器的 digest/权限/淘汰规则、时间范围校验，以及历史文件重定位与过滤。
// 由 formal 入口 `include!` 复用，使用正式 crate 名。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::time::Duration;
use astersql_util_stmtsummary_v2::*;

/// 构造仅含列名的 `ColumnInfo` 测试桩。
fn column(name: &str) -> model::ColumnInfo {
    let mut column = model::ColumnInfo::default();
    column.Name.O = name.to_owned();
    column
}

/// 校验平均值、空串转 nil、backoff 格式化及若干列工厂输出。
#[test]
fn column_helpers_and_factories_match_go() {
    assert_eq!(avgInt(9, 2), 4);
    assert_eq!(avgInt(9, 0), 0);
    assert_eq!(avgFloat(9, 2), 4.5);
    assert_eq!(avgSumFloat(1.5, 2), 0.75);
    assert_eq!(convertEmptyToNil(""), None);
    assert_eq!(convertEmptyToNil("db"), Some("db".to_owned()));
    assert_eq!(
        formatBackoffTypes(&HashMap::from([("txn".into(), 1), ("rpc".into(), 3)])),
        Some("rpc:3,txn:1".to_owned())
    );

    let mut record = StmtRecord::default();
    record.SchemaName = "test".into();
    record.ExecCount = 2;
    record.SumLatency = Duration::from_nanos(9);
    record.Prepared = true;
    record.BackoffTypes = HashMap::from([("rpc".into(), 2)]);
    let context = ColumnContext::new("127.0.0.1:4000", chrono_tz::UTC);
    let factories = makeColumnFactories(&[
        column(ClusterTableInstanceColumnNameStr),
        column(SchemaNameStr),
        column(AvgLatencyStr),
        column(PreparedStr),
        column(BackoffTypesStr),
    ]);
    let row: Vec<_> = factories
        .iter()
        .map(|factory| factory(&context, &record).into_datum())
        .collect();
    assert_eq!(row[0].GetString(), "127.0.0.1:4000");
    assert_eq!(row[1].GetString(), "test");
    assert_eq!(row[2].GetInt64(), 4);
    assert_eq!(row[3].GetInt64(), 1);
    assert_eq!(row[4].GetString(), "rpc:2");
}

/// 校验 marshal 扁平字段、evicted 标记、附加字段与 logEvicted 计数。
#[test]
fn logger_json_keeps_flat_record_and_markers() {
    let record = StmtRecord {
        Begin: 10,
        End: 20,
        Digest: "digest".into(),
        ..Default::default()
    };
    setStmtLogAdditionalFields(HashMap::new());
    let plain: serde_json::Value =
        serde_json::from_slice(&marshalStmtRecord(&record).unwrap()).unwrap();
    assert_eq!(plain["begin"], 10);
    assert!(plain.get("evicted").is_none());
    let evicted: serde_json::Value =
        serde_json::from_slice(&marshalEvictedStmtRecord(&record).unwrap()).unwrap();
    assert_eq!(evicted["digest"], "digest");
    assert_eq!(evicted["evicted"], true);

    setStmtLogAdditionalFields(HashMap::from([("keyspace_name".into(), "ks".into())]));
    let enriched: serde_json::Value =
        serde_json::from_slice(&marshalEvictedStmtRecord(&record).unwrap()).unwrap();
    assert_eq!(enriched["additional_fields"]["keyspace_name"], "ks");
    assert_eq!(encodeStmtLogEntry("message"), b"message\n");
    setStmtLogAdditionalFields(HashMap::new());

    let before = persistedEvictedCount();
    let mut storage = newStmtLogStorage(Vec::new());
    assert_eq!(storage.logEvicted([&record, &record]).unwrap(), 2);
    let output = String::from_utf8(storage.intoInner()).unwrap();
    assert_eq!(output.lines().count(), 2);
    assert!(output.lines().all(|line| line.contains("\"evicted\":true")));
    assert_eq!(persistedEvictedCount(), before + 2);
}

/// 测试用内存摘要源：固定返回单个窗口快照。
#[derive(Clone)]
struct TestMemorySource(MemWindowSnapshot);

impl MemorySummarySource for TestMemorySource {
    fn currentWindowSnapshot(&self) -> Option<MemWindowSnapshot> {
        Some(self.0.clone())
    }
}

/// 校验 MemReader 的权限、digest 过滤与淘汰行合并规则。
#[test]
fn memory_reader_matches_digest_privilege_and_evicted_rules() {
    let record = StmtRecord {
        Digest: "keep".into(),
        ExecCount: 3,
        AuthUsers: HashSet::from(["alice".into()]),
        ..Default::default()
    };
    let evicted = StmtRecord {
        Digest: "other".into(),
        ExecCount: 4,
        AuthUsers: HashSet::from(["alice".into()]),
        ..Default::default()
    };
    let source = TestMemorySource(MemWindowSnapshot {
        begin: 10,
        records: vec![record],
        evicted: Some(evicted),
    });
    let columns = [column(DigestStr), column(ExecCountStr)];

    // 无 digest 过滤时普通行 + 淘汰行共两行。
    let reader = NewMemReader(
        Some(&source),
        &columns,
        "",
        chrono_tz::UTC,
        Some("alice".into()),
        false,
        None,
        vec![],
    );
    assert_eq!(reader.Rows().len(), 2);

    // 限定 digest 集合后只保留匹配行，且不附加淘汰行。
    let reader = NewMemReader(
        Some(&source),
        &columns,
        "",
        chrono_tz::UTC,
        Some("alice".into()),
        false,
        Some(HashSet::from(["keep".into()])),
        vec![],
    );
    let rows = reader.Rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0].GetString(), "keep");
}

/// 校验时间区间重叠边界、StmtChecker 权限/digest/时间与 needStop。
#[test]
fn checker_and_time_ranges_keep_go_boundaries() {
    assert!(!timeRangeOverlap(1, 2, 3, 4));
    assert!(timeRangeOverlap(1, 2, 2, 3));
    assert!(timeRangeOverlap(1, 0, 3, 4));
    let checker = StmtChecker::new(
        Some("alice".into()),
        false,
        Some(HashSet::from(["digest".into()])),
        vec![StmtTimeRange { Begin: 10, End: 20 }],
    );
    assert!(checker.hasPrivilege(&HashSet::from(["alice".into()])));
    assert!(!checker.hasPrivilege(&HashSet::from(["bob".into()])));
    assert!(checker.isDigestValid("digest"));
    assert!(checker.isTimeValid(20, 30));
    assert!(checker.needStop(21));
}

/// 校验 openStmtFile 重定位、历史读取跳过非法行与 evicted 行。
#[test]
fn statement_file_reseeks_and_history_filters_invalid_and_evicted_lines() {
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("tidb-statements.log");
    let rotated = dir
        .path()
        .join("tidb-statements-2022-12-27T16-21-20.245.log");
    fs::write(
        &rotated,
        "invalid\n{\"begin\":1,\"end\":2,\"digest\":\"one\",\"exec_count\":1}\n",
    )
    .unwrap();
    fs::write(
        &active,
        "{\"begin\":3,\"end\":4,\"digest\":\"two\",\"exec_count\":2}\n\
         {\"begin\":5,\"end\":6,\"digest\":\"skip\",\"exec_count\":9,\"evicted\":true}\n\
         corrupted\n",
    )
    .unwrap();
    setStmtSummaryFilename(active.clone());

    // 打开轮转文件：begin 取首条合法 JSON，end 从文件名时间戳解析，随后 seek 回起点。
    let mut opened = openStmtFile(&rotated).unwrap();
    assert_eq!(opened.begin, 1);
    let expected_end =
        chrono::NaiveDateTime::parse_from_str("2022-12-27T16-21-20.245", logFileTimeFormat)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap()
            .timestamp();
    assert_eq!(opened.end, expected_end);
    opened.file.seek(SeekFrom::Start(0)).unwrap();
    let mut first = String::new();
    BufReader::new(&mut opened.file)
        .read_line(&mut first)
        .unwrap();
    assert_eq!(first.trim_end(), "invalid");

    let mut reader = NewHistoryReader(
        &[column(DigestStr), column(ExecCountStr)],
        "",
        chrono_tz::UTC,
        None,
        false,
        None,
        vec![],
        2,
    )
    .unwrap();
    let mut rows = Vec::new();
    while let Some(batch) = reader.Rows().unwrap() {
        rows.extend(batch);
    }
    reader.Close().unwrap();
    assert_eq!(rows.len(), 2);
    let mut digests: Vec<_> = rows.iter().map(|row| row[0].GetString()).collect();
    digests.sort();
    assert_eq!(digests, ["one", "two"]);
}

#[test]
fn plan_column_keeps_go_string_bytes() {
    let mut record = StmtRecord::default();
    record.SamplePlan = plancodec_dependency::Compress(b"0\t1\t0\t\xff");
    let context = ColumnContext::new("", chrono_tz::UTC);
    let factories = makeColumnFactories(&[column(PlanStr)]);
    let datum = factories[0](&context, &record).into_datum();
    assert!(datum.GetBytes().ends_with(b"\xff"));
    let text = types::NewStringDatum(String::new());
    assert_eq!(datum.Kind(), text.Kind());
    assert_eq!(datum.Collation(), text.Collation());
}
