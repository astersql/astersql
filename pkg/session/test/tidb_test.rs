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

// Session 层 TiDB 兼容辅助逻辑的轻量测试。
//
// 覆盖解析 warning / KeyNeedToLock（事务提交前判断某 key 是否需要加悲观/乐观锁），
// 并用 mock RecordSet 验证 `GetRows4Test` 按 chunk 排空结果集。

use astersql_session::SessionError;
use astersql_session::SessionResult;
use astersql_session::tidb::{
    CellValue, GetRows4Test, Parse, ParseRuntime, ParsedStatement, RecordSetRuntime, Row,
    StatementKind,
};
use astersql_session::txn::{KeyFlags, KeyNeedToLock};

#[derive(Default)]
/// 与 Go `TestParseErrorWarn` 对应的可控 parser：成功时返回一条 warning，失败时保留原错误。
struct WarningParser {
    warnings: Vec<String>,
    fail: bool,
}

impl ParseRuntime for WarningParser {
    fn ParseSQL(&mut self, source: &str) -> SessionResult<(Vec<ParsedStatement>, Vec<String>)> {
        if self.fail {
            return Err(SessionError::new("parse failed"));
        }
        Ok((
            vec![ParsedStatement {
                text: source.to_owned(),
                kind: StatementKind::Other,
                read_only: true,
            }],
            vec!["deprecated hint".to_owned()],
        ))
    }

    fn AppendWarning(&mut self, warning: String) {
        self.warnings.push(warning);
    }
}

/// 对齐 Go `TestParseErrorWarn`：成功解析保留一条 warning，非法 SQL 不吞掉 parse 错误。
#[test]
fn parse_error_warn_forwards_warning_and_preserves_error() {
    let mut parser = WarningParser::default();
    let statements = Parse(&mut parser, "select /*+ adf */ 1").unwrap();
    assert_eq!(statements.len(), 1);
    assert_eq!(statements[0].text, "select /*+ adf */ 1");
    assert_eq!(parser.warnings, ["deprecated hint"]);

    parser.fail = true;
    assert_eq!(
        Parse(&mut parser, "select").unwrap_err().to_string(),
        "parse failed"
    );
}

/// 对齐 Go `TestKeysNeedLock` 的所有判定分支。
///
/// Rust 的 transaction backend 已在 `KeyFlags` 中解析 key/value 分类，因此这里直接构造
/// 与 Go 表驱动用例等价的分类结果，而不是重复实现 tablecodec 解码。
#[test]
fn keys_need_lock_matches_go_branch_matrix() {
    let table_key = |flags: KeyFlags| KeyFlags {
        table_key: true,
        ..flags
    };

    assert!(KeyNeedToLock(b"meta", &KeyFlags::default()));
    assert!(!KeyNeedToLock(
        b"row",
        &table_key(KeyFlags {
            need_constraint_check_in_prewrite: true,
            ..KeyFlags::default()
        })
    ));
    assert!(KeyNeedToLock(
        b"row",
        &table_key(KeyFlags {
            presume_key_not_exists: true,
            ..KeyFlags::default()
        })
    ));
    assert!(KeyNeedToLock(
        b"",
        &table_key(KeyFlags {
            record_key: true,
            ..KeyFlags::default()
        })
    ));
    assert!(!KeyNeedToLock(
        b"",
        &table_key(KeyFlags {
            index_key: true,
            ..KeyFlags::default()
        })
    ));
    assert!(!KeyNeedToLock(
        b"index",
        &table_key(KeyFlags {
            index_key: true,
            untouched_index_value: true,
            ..KeyFlags::default()
        })
    ));
    assert!(KeyNeedToLock(b"row", &table_key(KeyFlags::default())));
    assert!(!KeyNeedToLock(
        b"index",
        &table_key(KeyFlags {
            index_key: true,
            ..KeyFlags::default()
        })
    ));
    assert!(KeyNeedToLock(
        b"index",
        &table_key(KeyFlags {
            index_key: true,
            need_locked: true,
            ..KeyFlags::default()
        })
    ));
    assert!(KeyNeedToLock(
        b"unique-index",
        &table_key(KeyFlags {
            index_key: true,
            index_value_is_unique: true,
            ..KeyFlags::default()
        })
    ));
    assert!(!KeyNeedToLock(
        b"temp-index",
        &table_key(KeyFlags {
            index_key: true,
            temp_index_key: true,
            ..KeyFlags::default()
        })
    ));
    assert!(KeyNeedToLock(
        b"temp-index",
        &table_key(KeyFlags {
            index_key: true,
            temp_index_key: true,
            next_gen: true,
            ..KeyFlags::default()
        })
    ));
}

/// 验证 `GetRows4Test` 反复调用 `Next` 直至空 chunk，并收集全部行。
#[test]
fn tidb_result_reader_drains_chunks_until_empty() {
    /// 仅产生两行后返回空的简易 RecordSet，用于模拟分 chunk 输出。
    struct Records(usize);
    impl RecordSetRuntime for Records {
        fn Next(&mut self, chunk: &mut Vec<Row>) -> SessionResult {
            if self.0 < 2 {
                chunk.push(Row {
                    cells: vec![CellValue::Text(self.0.to_string())],
                });
                self.0 += 1;
            }
            Ok(())
        }
        fn Close(&mut self) -> SessionResult {
            Ok(())
        }
    }
    let mut records = Records(0);
    let rows = GetRows4Test(Some(&mut records)).unwrap();
    assert_eq!(rows.len(), 2);
}
