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

// session 包测试入口与行结果匹配辅助。
//
// 对应 Go `TestMain`：配置公共测试环境、schema lease、异步提交窗口，
// 并保留可执行的配置与 `match` 辅助逻辑。

use std::time::Duration;
/// DatumDraft 对应 Go 的 types.Datum 在本测试辅助函数中的显示语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatumDraft {
    /// 列值的字符串显示形式。
    pub value: String,
}

/// ExpectedValue 保留 Go `match` 中字符串比较和时间戳跳过两类输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedValue {
    /// 按字符串精确比较的期望列值。
    Text(String),
    /// 时间戳类列：跳过比较（对应默认 current_timestamp）。
    TimeVariant,
}

impl From<&str> for ExpectedValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for ExpectedValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

/// 应用 Go `TestMain` 中可由 Rust 测试框架复现的进程级配置。
fn configure_test_environment() {
    astersql_testkit_testsetup::SetupForCommonTest();
    astersql_sessionctx_vardef::SetSchemaLease(Duration::from_millis(20));
    astersql_config::update_global(|config| {
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
    });
}

/// `test_main` 对应 Go `TestMain` 的可执行部分。
///
/// Rust libtest 没有 Go 的 `*testing.M` 进程入口；这里执行可观察的配置。
pub fn test_main() {
    configure_test_environment();
}
/// `test_main_callback` 对应 Go 传给 `testmain.WrapTestingM` 的 callback。
pub fn test_main_callback(i: i32) -> i32 {
    // wait for MVCCLevelDB to close, MVCCLevelDB will be closed in one second
    std::thread::sleep(Duration::from_secs(1));
    i
}

/// `match_row` 对应 Go `match(t, row, expected...)`：逐列比较，时间值跳过。
pub fn match_row(row: &[DatumDraft], expected: &[ExpectedValue]) {
    assert_eq!(row.len(), expected.len(), "Go require.Len 对应的长度检查");
    for (idx, datum) in row.iter().enumerate() {
        match &expected[idx] {
            ExpectedValue::TimeVariant => {
                // Since password_last_changed is set to default current_timestamp, we pass this check.
                continue;
            }
            ExpectedValue::Text(need) => {
                // Go 使用 fmt.Sprintf("%v", row[i].GetValue()) 与 fmt.Sprintf("%v", expected[i]) 比较。
                assert_eq!(need, &datum.value, "row index {idx}");
            }
        }
    }
}

/// 简易 RecordSet：按批吐出行，用于验证 ResultSetToStringSlice 的关闭与 NULL 显示。
struct CanonicalRecordSet {
    batches: Vec<Vec<crate::tidb::Row>>,
    closed: bool,
}

impl crate::tidb::RecordSetRuntime for CanonicalRecordSet {
    fn Next(&mut self, chunk: &mut Vec<crate::tidb::Row>) -> crate::SessionResult {
        if !self.batches.is_empty() {
            chunk.extend(self.batches.remove(0));
        }
        Ok(())
    }

    fn Close(&mut self) -> crate::SessionResult {
        self.closed = true;
        Ok(())
    }
}

/// 验证结果集转字符串切片保留 NULL 为 `<nil>`，并在结束后关闭 RecordSet。
#[test]
fn canonical_result_set_conversion_preserves_null_and_closes() {
    use crate::tidb::{CellValue, ResultSetToStringSlice, Row};

    let mut record_set = CanonicalRecordSet {
        batches: vec![
            vec![Row {
                cells: vec![CellValue::Text("v".into()), CellValue::Null],
            }],
            Vec::new(),
        ],
        closed: false,
    };
    assert_eq!(
        ResultSetToStringSlice(&mut record_set).unwrap(),
        vec![vec!["v".to_owned(), "<nil>".to_owned()]]
    );
    assert!(record_set.closed);
}

#[test]
fn test_main_environment_applies_go_schema_and_async_commit_settings() {
    let previous_lease = astersql_sessionctx_vardef::GetSchemaLease();
    let restore_config = astersql_config::restore_func();

    configure_test_environment();

    assert_eq!(
        astersql_sessionctx_vardef::GetSchemaLease(),
        std::time::Duration::from_millis(20)
    );
    let config = astersql_config::get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);

    astersql_sessionctx_vardef::SetSchemaLease(previous_lease);
    restore_config();
}

#[test]
fn get_db_names_matches_go_fallback_deduplication_and_label_gate() {
    use astersql_sessionctx_stmtctx::TableEntry;

    let restore_config = astersql_config::restore_func();
    let session_vars = astersql_sessionctx_variable::session::SessionVars::new();
    session_vars.SetCurrentDB("DatabaseA");

    astersql_config::update_global(|config| config.status.record_db_label = false);
    assert_eq!(crate::GetDBNames(Some(&session_vars)), vec![""]);
    assert_eq!(crate::GetDBNames(None), vec![""]);

    astersql_config::update_global(|config| config.status.record_db_label = true);
    assert_eq!(crate::GetDBNames(Some(&session_vars)), vec!["databasea"]);

    session_vars.StmtCtx.SetLogicalPlanTables(vec![
        TableEntry {
            DB: "db_b".into(),
            Table: "t2".into(),
        },
        TableEntry {
            DB: "db_a".into(),
            Table: "t1".into(),
        },
        TableEntry {
            DB: "db_b".into(),
            Table: "t3".into(),
        },
    ]);
    assert_eq!(
        crate::GetDBNames(Some(&session_vars)),
        vec!["db_a".to_owned(), "db_b".to_owned()]
    );

    restore_config();
}

#[test]
fn match_row_compares_text_and_skips_time_variants() {
    match_row(
        &[
            DatumDraft { value: "42".into() },
            DatumDraft {
                value: "runtime timestamp".into(),
            },
        ],
        &[ExpectedValue::from("42"), ExpectedValue::TimeVariant],
    );
}

#[test]
fn test_main_callback_waits_for_mvcc_cleanup_and_preserves_exit_code() {
    let started = std::time::Instant::now();
    assert_eq!(test_main_callback(7), 7);
    assert!(started.elapsed() >= Duration::from_secs(1));
}

#[test]
fn session_error_preserves_display_equality_and_shared_source() {
    let source = astersql_errors::New("storage unavailable");
    let error = crate::SessionError::with_source("session failed", source);

    assert_eq!(error.to_string(), "session failed");
    assert_eq!(error, crate::SessionError::new("session failed"));
    assert_eq!(error.into_shared().to_string(), "storage unavailable");
    assert_eq!(
        crate::SessionError::new("plain failure")
            .into_shared()
            .to_string(),
        "plain failure"
    );
}
