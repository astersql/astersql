// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 一致性报告器迁移补充单元测试。
//
// 覆盖 `GetMvccByKey` 的 JSON/截断、行与索引 MVCC（多版本并发控制）
// 解码，以及 Lookup / AdminCheck 报告在脱敏开关下的字段行为，对齐 Go。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use model;
use tablecodec;

use model::{ColumnInfo, IndexInfo, TableInfo};
use serde_json::Value;
use tablecodec::kv::{Handle, IntHandle, Key};
use tablecodec::types::{NewIntDatum, NewStringDatum};
use crate::{
    ConsistencyErrorKind, DecodeIndexMvccData, DecodeRowMvccData, GetMvccByKey, LogEntry, LogSink,
    MvccGetByKeyResponse, MvccInfo, MvccOutMap, MvccValue, MvccWrite, RecordData, Reporter,
    Storage,
};

/// 可预设响应/Region 并记录查询键的假存储，用于隔离存储依赖。
#[derive(Clone)]
struct MockStorage {
    /// `get_mvcc_by_encoded_key` 的固定返回值。
    response: Result<MvccGetByKeyResponse, String>,
    /// `region_id_by_key` 的固定返回值。
    region: Result<u64, String>,
    /// 记录每次 MVCC 查询使用的编码键。
    calls: Arc<Mutex<Vec<Key>>>,
}

impl MockStorage {
    /// 构造成功路径：固定 MVCC 响应与 Region ID。
    fn successful(response: MvccGetByKeyResponse, region: u64) -> Self {
        Self {
            response: Ok(response),
            region: Ok(region),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 构造失败路径：MVCC 与 Region 查询均返回错误。
    fn failing() -> Self {
        Self {
            response: Err("mvcc unavailable".to_string()),
            region: Err("region unavailable".to_string()),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Storage for MockStorage {
    fn get_mvcc_by_encoded_key(&self, key: &Key) -> Result<MvccGetByKeyResponse, String> {
        self.calls.lock().unwrap().push(key.clone());
        self.response.clone()
    }

    fn region_id_by_key(&self, _key: &Key) -> Result<u64, String> {
        self.region.clone()
    }
}

/// 收集 `LogSink::error` 写入的日志条目，供断言字段内容。
#[derive(Default)]
struct ObservedLogger {
    entries: Mutex<Vec<LogEntry>>,
}

impl LogSink for ObservedLogger {
    fn error(&self, entry: LogEntry) {
        self.entries.lock().unwrap().push(entry);
    }
}

/// 构造含一列 longlong 的表与对应索引元数据。
fn table_and_index() -> (TableInfo, IndexInfo) {
    let mut column = ColumnInfo::New(1, model::ast::NewCIStr("c"));
    column.SetType(model::mysql::TypeLonglong);
    let table = TableInfo {
        ID: 11,
        Name: model::ast::NewCIStr("t"),
        Columns: vec![column],
        ..Default::default()
    };
    let index = IndexInfo {
        ID: 22,
        Name: model::ast::NewCIStr("idx"),
        ..Default::default()
    };
    (table, index)
}

/// 构造含 Writes/Values 的样例 MVCC 响应。
fn response_with_values() -> MvccGetByKeyResponse {
    MvccGetByKeyResponse {
        Info: Some(MvccInfo {
            Writes: vec![MvccWrite {
                StartTs: 7,
                CommitTs: 9,
                ShortValue: vec![1, 2, 3],
                ..Default::default()
            }],
            Values: vec![MvccValue {
                StartTs: 8,
                Value: vec![4, 5, 6],
            }],
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// 验证 nil 键、JSON 解码字段、诊断失败非致命，以及超长输出截断。
#[test]
fn get_mvcc_by_key_matches_go_nil_error_json_decode_and_truncation() {
    let storage = MockStorage::successful(response_with_values(), 88);
    assert_eq!(GetMvccByKey(&storage, None, None), "");

    let decode = |_key: &Key, _response: &MvccGetByKeyResponse, out: &mut MvccOutMap| {
        out.insert("decoded".to_string(), serde_json::json!({"7": {"c": "42"}}));
    };
    let output = GetMvccByKey(&storage, Some(Key(vec![0xab, 0xcd])), Some(&decode));
    let json: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(json["key"], "ABCD");
    assert_eq!(json["regionID"], 88);
    assert_eq!(json["decoded"]["7"]["c"], "42");
    assert!(json["mvcc"]["info"].is_object());

    // Go 将诊断用 MVCC 失败视为非致命，返回空串而非错误
    let failing = MockStorage::failing();
    assert_eq!(
        GetMvccByKey(&failing, Some(Key(vec![1])), None),
        "",
        "Go treats diagnostic MVCC failures as non-fatal"
    );

    // 超长 decoded 字段应截断到 5000 并追加标记
    let long_decode = |_key: &Key, _response: &MvccGetByKeyResponse, out: &mut MvccOutMap| {
        out.insert("decoded".to_string(), Value::String("x".repeat(6_000)));
    };
    let truncated = GetMvccByKey(&storage, Some(Key(vec![0xab, 0xcd])), Some(&long_decode));
    assert_eq!(truncated.len(), 5_000 + "[truncated]...".len());
    assert!(truncated.ends_with("[truncated]..."));
}

/// 验证 MVCC protobuf 字段沿用 Go `encoding/json` 的 omitempty 与 []byte Base64 语义。
#[test]
fn mvcc_json_matches_go_protobuf_tags_and_byte_encoding() {
    let storage = MockStorage::successful(response_with_values(), 88);
    let decode = |_key: &Key, _response: &MvccGetByKeyResponse, out: &mut MvccOutMap| {
        out.insert(
            "html".to_string(),
            Value::String("<&>\u{2028}\u{2029}".to_string()),
        );
    };
    let output = GetMvccByKey(&storage, Some(Key(vec![0xab, 0xcd])), Some(&decode));
    let json: Value = serde_json::from_str(&output).unwrap();

    assert!(output.contains(r#""html":"\u003c\u0026\u003e\u2028\u2029""#));
    assert_eq!(
        json["mvcc"],
        serde_json::json!({
            "info": {
                "writes": [{
                    "start_ts": 7,
                    "commit_ts": 9,
                    "short_value": "AQID"
                }],
                "values": [{
                    "start_ts": 8,
                    "value": "BAUG"
                }]
            }
        })
    );
    let decoded: MvccGetByKeyResponse = serde_json::from_value(json["mvcc"].clone()).unwrap();
    let info = decoded.Info.unwrap();
    assert_eq!(info.Writes[0].ShortValue, vec![1, 2, 3]);
    assert_eq!(info.Values[0].Value, vec![4, 5, 6]);
}

/// 验证行/索引 MVCC 解码对 write、value 与 decode_error 的处理对齐 Go。
#[test]
fn row_and_index_mvcc_decoders_match_go_write_value_and_error_behavior() {
    let (table, mut index) = table_and_index();
    let encoded_row = tablecodec::EncodeRow(
        tablecodec::codec::NewEncoder(false),
        Some(tablecodec::time::UTC),
        vec![NewIntDatum(42)],
        vec![1],
        Vec::new(),
        None,
        None,
        tablecodec::rowcodec::Encoder::new(false),
    )
    .unwrap();
    let row_response = MvccGetByKeyResponse {
        Info: Some(MvccInfo {
            Writes: vec![MvccWrite {
                StartTs: 7,
                ShortValue: encoded_row,
                ..Default::default()
            }],
            Values: vec![MvccValue {
                StartTs: 8,
                Value: vec![0xff],
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut row_out = MvccOutMap::new();
    DecodeRowMvccData(&table)(&Key(vec![]), &row_response, &mut row_out);
    assert_eq!(row_out["decoded"]["7"]["c"], "42");
    assert!(row_out["decoded"]["8"].as_object().unwrap().is_empty());
    assert!(row_out["decode_error"].as_str().unwrap().len() > 3);

    // 无列索引：从唯一索引 value 解出 handle
    index.Columns.clear();
    let index_key = tablecodec::EncodeIndexSeekKey(11, 22, None);
    let handle_value = tablecodec::EncodeHandleInUniqueIndexValue(Box::new(IntHandle(123)), false);
    let index_response = MvccGetByKeyResponse {
        Info: Some(MvccInfo {
            Writes: vec![MvccWrite {
                StartTs: 10,
                ShortValue: handle_value,
                ..Default::default()
            }],
            Values: vec![MvccValue {
                StartTs: 11,
                Value: vec![0],
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut index_out = MvccOutMap::new();
    DecodeIndexMvccData(&index)(&index_key, &index_response, &mut index_out);
    assert_eq!(index_out["decoded"]["10"]["handle"], "123");
    assert!(index_out["decoded"].get("11").is_none());
    assert!(index_out["decode_error"].is_string());
}

/// 用固定表/索引编码闭包组装 `Reporter`。
fn build_reporter(mode: &str, storage: Arc<MockStorage>, logger: Arc<ObservedLogger>) -> Reporter {
    let (table, index) = table_and_index();
    Reporter::new(
        Arc::new(|handle: &dyn Handle| Key(handle.Encoded())),
        Arc::new(|_row: &RecordData| tablecodec::EncodeIndexSeekKey(11, 22, None)),
        table,
        index,
        mode.to_string(),
        Some(storage),
        logger,
    )
}

/// 将日志字段列表转为键值映射，便于按名断言。
fn field_map(entry: &LogEntry) -> HashMap<String, String> {
    entry
        .Fields
        .iter()
        .map(|field| (field.Key.clone(), field.Value.clone()))
        .collect()
}

/// 验证 Lookup 不一致报告的 handle 截断、MVCC 字段与 ON 脱敏。
#[test]
fn lookup_report_matches_go_handle_limit_redaction_mvcc_and_error_behavior() {
    let storage = Arc::new(MockStorage::successful(response_with_values(), 88));
    let logger = Arc::new(ObservedLogger::default());
    let reporter = build_reporter("OFF", storage.clone(), logger.clone());
    let missing: Vec<Box<dyn Handle>> = vec![Box::new(IntHandle(1))];
    // 超过 maxFullHandleCnt(50) 时仅展示前 50 个
    let full: Vec<Box<dyn Handle>> = (0..60)
        .map(|value| Box::new(IntHandle(value)) as Box<dyn Handle>)
        .collect();
    let missing_indexes = vec![RecordData::new(
        Box::new(IntHandle(9)),
        vec![NewStringDatum("v".to_string())],
    )];

    let error = reporter.ReportLookupInconsistent(2, 1, &missing, &full, &missing_indexes);
    assert_eq!(error.Kind, ConsistencyErrorKind::LookupMismatchCount);
    assert_eq!(error.Args, vec!["t", "idx", "2", "1"]);
    assert_eq!(
        error.to_string(),
        "[executor:8133]data inconsistency in table: t, index: idx, index-count:2 != record-count:1"
    );
    let entries = logger.entries.lock().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].Message, "indexLookup found data inconsistency");
    let fields = field_map(&entries[0]);
    assert_eq!(fields["missing_handles"], "[1]");
    assert!(fields["total_handles"].ends_with(" 49]"));
    assert!(!fields["total_handles"].contains(" 50]"));
    assert!(fields.contains_key("row_mvcc_0"));
    assert!(fields.contains_key("index_mvcc_0"));
    drop(entries);

    // EnableRedactLog=ON：清空敏感字段且不再发起 MVCC 查询
    let redacted_logger = Arc::new(ObservedLogger::default());
    let calls_before = storage.calls.lock().unwrap().len();
    let redacted = build_reporter("ON", storage.clone(), redacted_logger.clone());
    redacted.ReportLookupInconsistent(2, 1, &missing, &full, &missing_indexes);
    assert_eq!(storage.calls.lock().unwrap().len(), calls_before);
    let redacted_entries = redacted_logger.entries.lock().unwrap();
    let redacted_fields = field_map(&redacted_entries[0]);
    assert_eq!(redacted_fields["missing_handles"], "");
    assert!(!redacted_fields.contains_key("row_mvcc_0"));
}

/// 验证 AdminCheck 报告的元数据字段、列级错误参数与 nil 索引行行为。
#[test]
fn admin_reports_match_go_metadata_redaction_nil_index_and_column_error_behavior() {
    let storage = Arc::new(MockStorage::successful(response_with_values(), 88));
    let logger = Arc::new(ObservedLogger::default());
    let reporter = build_reporter("ON", storage, logger.clone());
    let handle = IntHandle(5);
    let index_row = RecordData::new(Box::new(IntHandle(5)), vec![NewIntDatum(6)]);
    let table_row = RecordData::new(Box::new(IntHandle(5)), vec![NewIntDatum(7)]);
    assert_eq!(
        index_row.String(),
        "handle: 5, values: [KindInt64 6]"
    );

    let error = reporter.ReportAdminCheckInconsistent(&handle, Some(&index_row), Some(&table_row));
    assert_eq!(error.Kind, ConsistencyErrorKind::AdminCheck);
    assert_eq!(
        error.to_string(),
        "[admin:8223]data inconsistency in table: t, index: idx, handle: ?, index-values:\"?\" != record-values:\"?\""
    );
    let entries = logger.entries.lock().unwrap();
    let fields = field_map(&entries[0]);
    assert_eq!(fields["int_handle"], "5");
    assert!(fields.contains_key("row_mvcc_write_0"));
    assert!(fields.contains_key("index_mvcc_value_0"));
    assert!(!fields["row_mvcc_write_0"].contains("\"short_value\""));
    assert!(!fields["index_mvcc_value_0"].contains("\"value\""));
    assert!(!fields.contains_key("row_mvcc"));
    drop(entries);

    let col_error = reporter.ReportAdminCheckInconsistentWithColInfo(
        &handle,
        "c",
        "6",
        "7",
        "different values",
        &index_row,
    );
    assert_eq!(
        col_error.Kind,
        ConsistencyErrorKind::AdminCheckWithColumnInfo
    );
    assert_eq!(
        col_error.Args,
        vec!["t", "idx", "c", "5", "6", "7", "different values"]
    );
    assert_eq!(
        col_error.to_string(),
        "[executor:8134]data inconsistency in table: t, index: idx, col: c, handle: \"?\", index-values:\"?\" != record-values:\"?\", compare err:\"?\""
    );
    let entries = logger.entries.lock().unwrap();
    let col_fields = field_map(&entries[1]);
    assert_eq!(col_fields["error"], "different values");
    assert!(!col_fields.contains_key("row_mvcc"));

    drop(entries);
    // nil 索引/表行时不应写出 index_mvcc_write_* 字段
    reporter.ReportAdminCheckInconsistent(&handle, None, None);
    let entries = logger.entries.lock().unwrap();
    let nil_fields = field_map(&entries[2]);
    assert!(!nil_fields.contains_key("index_mvcc_write_0"));
}
