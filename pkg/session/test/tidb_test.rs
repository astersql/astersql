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
// 覆盖解析 warning / KeyNeedToLock（事务提交前判断某 key 是否需要加悲观/乐观锁）等
// Go 草稿流程；可执行部分用 mock RecordSet 验证 `GetRows4Test` 按 chunk 排空结果集。

// 这段逻辑只描述 parser warning 与 KeyNeedToLock 的测试流程，不连接真实数据库，也不执行 TiDB 业务动作。
//
// test_parse_error_warn 对应 Go 的 TestParseErrorWarn。
// 它用 mock session context 验证 hint 解析 warning 与非法 SQL parse error；defer 关闭 stats handle 的资源收尾语义保留在注释中。
// #[test]
// pub fn test_parse_error_warn() {
//     let ctx = coretestsdk::MockContext();
// Go defer: domain.GetDomain(ctx).StatsHandle().Close()，不实际持有 domain 资源。
//     defer_stats_handle_close(|| domain::GetDomain(&ctx).StatsHandle().Close());
//
//     let (nodes, err) = session::Parse(&ctx, "select /*+ adf */
//  1");
//     require::NoError(err);
//     require::Len(&nodes, 1);
//     require::Len(ctx.GetSessionVars().StmtCtx.GetWarnings(), 1);
//
//     let (_, err) = session::Parse(&ctx, "select");
//     require::Error(err);
// }
//
// key_need_lock_case 对应 Go 的匿名测试表，保留 key/value 与预期 need-lock 的组合。
// struct KeyNeedLockCase {
//     key: Vec<u8>,
//     val: Vec<u8>,
//     need: bool,
// }
//
// test_keys_need_lock 对应 Go 的 TestKeysNeedLock。
// 该测试覆盖行 key、唯一/非唯一索引 key、删除 tombstone、presume-not-exists flag，以及临时索引 value 的分支。
// #[test]
// pub fn test_keys_need_lock() {
//     let row_key = tablecodec::EncodeRowKeyWithHandle(1, kv::IntHandle(1));
//     let unique_index_key = tablecodec::EncodeIndexSeekKey(1, 1, vec![1]);
//     let non_unique_index_key = tablecodec::EncodeIndexSeekKey(1, 2, vec![1]);
//     let mut temp_index_key = tablecodec::EncodeIndexSeekKey(1, 3, vec![1]);
//     tablecodec::IndexKey2TempIndexKey(&mut temp_index_key);
//
//     let unique_value = vec![0; 8];
//     let mut unique_untouched = unique_value.clone();
//     unique_untouched.push(b'1');
//     let non_unique_val = vec![b'0'];
//     let non_unique_untouched = vec![b'1'];
//     let delete_val: Vec<u8> = Vec::new();
//     let row_val = vec![b'a', b'b', b'c'];
//
//     let tests = vec![
//         KeyNeedLockCase { key: row_key.clone(), val: row_val.clone(), need: true },
//         KeyNeedLockCase { key: row_key.clone(), val: delete_val.clone(), need: true },
//         KeyNeedLockCase { key: non_unique_index_key.clone(), val: non_unique_val.clone(), need: false },
//         KeyNeedLockCase { key: non_unique_index_key.clone(), val: non_unique_untouched.clone(), need: false },
//         KeyNeedLockCase { key: unique_index_key.clone(), val: unique_value.clone(), need: true },
//         KeyNeedLockCase { key: unique_index_key.clone(), val: unique_untouched.clone(), need: false },
//         KeyNeedLockCase { key: unique_index_key.clone(), val: delete_val.clone(), need: false },
//     ];
//
//     for case in tests {
//         let need = session::KeyNeedToLock(&case.key, &case.val, kv::KeyFlags(0));
//         require::Equal(case.need, need);
//
// Go 这里传入非零 flag 后会写入 PresumeKeyNotExists，并强制返回需要加锁。
//         let mut flag = kv::KeyFlags(1);
//         let need = session::KeyNeedToLock(&case.key, &case.val, flag);
//         require::True(flag.HasPresumeKeyNotExists());
//         require::True(need);
//
// 非唯一索引的普通 value 如果外部已经设置 NeedLocked flag，也必须被判定为加锁。
//         if bytes::Equal(&case.key, &non_unique_index_key) && bytes::Equal(&case.val, &non_unique_val) {
//             flag = kv::ApplyFlagsOps(kv::KeyFlags(0), kv::SetNeedLocked);
//             require::True(flag.HasNeedLocked());
//             require::True(session::KeyNeedToLock(&case.key, &case.val, flag));
//         }
//     }
//
//     let temp_idx_value = tablecodec::TempIndexValueElem {
//         Value: non_unique_val,
//         KeyVer: tablecodec::TempIndexKeyTypeBackfill,
//     }.Encode(None);
//     let need = session::KeyNeedToLock(&temp_index_key, &temp_idx_value, kv::KeyFlags(0));
//     if kerneltype::IsNextGen() {
//         require::True(need);
//     } else {
//         require::False(need);
//     }
//
//     let flag = kv::KeyFlags(1);
//     require::True(session::KeyNeedToLock(&temp_index_key, &temp_idx_value, flag));
// }
// */
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
