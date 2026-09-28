// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/restore/utils/rewrite_rule_test.go`.
//! Algorithm + concurrency over shared rules — no TiKV / PD / network.
//!
//! 对齐 Go `rewrite_rule_test.go`：覆盖文件规则校验、SST/日志键重写、
//! 区间 RewriteRange、表 ID 解析、规则生成（粗/细粒度与 Map）、
//! 匹配查找、跨表错误、CF 时间过滤及并发只读合约。
//! 约束：仅解释测试意图与断言依据，不改输入/期望/并发结构。

// 标准库：HashMap 承载索引 ID 映射；mpsc/thread 用于竞态用例。
use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;

// ErrRestoreTableIDMismatch：区间重写发现起止表 ID 不一致时使用。
use astersql_br_pkg_errors::ErrRestoreTableIDMismatch;
// KeyRange/Range：RewriteRange 输入输出载体。
use astersql_br_pkg_rtree::{KeyRange, Range};
// Annotate 包装共享错误，保持与 Go errors.Annotate 文案一致。
use astersql_errors::{Annotate, SharedError};

// 桩类型：避免本测试依赖真实 kvproto。
use crate::stubs::{AppliedFile, backuppb, codec, import_sstpb, model, tablecodec};
// 被测公开 API：规则生成、匹配、键重写与时间过滤。
use crate::{
    FindMatchedRewriteRule, GetRewriteEncodedKeys, GetRewriteRawKeys, GetRewriteRuleOfTable,
    GetRewriteRules, GetRewriteRulesMap, GetRewriteTableID, RewriteAndEncodeRawKey, RewriteRange,
    RewriteRules, SetTimeRangeFilter, TableIDRemap, ValidateFileRewriteRule,
};

/// Go `RewriteRules.Clone` deliberately copies checkpoint/keyspace data but
/// leaves the time-range fields at their zero values.
#[test]
fn test_clone_matches_go_field_selection() {
    let rules = RewriteRules {
        Data: vec![import_sstpb::RewriteRule {
            OldKeyPrefix: b"old".to_vec(),
            NewKeyPrefix: b"new".to_vec(),
            NewTimestamp: 42,
            ..Default::default()
        }],
        OldKeyspace: b"old-keyspace".to_vec(),
        NewKeyspace: b"new-keyspace".to_vec(),
        NewTableID: 9,
        ShiftStartTs: 10,
        StartTs: 20,
        RestoredTs: 30,
        TableIDRemapHint: vec![TableIDRemap {
            Origin: 1,
            Rewritten: 9,
        }],
    };

    let cloned = rules.Clone();
    assert_eq!(cloned.Data, rules.Data);
    assert_eq!(cloned.OldKeyspace, rules.OldKeyspace);
    assert_eq!(cloned.NewKeyspace, rules.NewKeyspace);
    assert_eq!(cloned.NewTableID, rules.NewTableID);
    assert_eq!(cloned.TableIDRemapHint, rules.TableIDRemapHint);
    assert_eq!(cloned.ShiftStartTs, 0);
    assert_eq!(cloned.StartTs, 0);
    assert_eq!(cloned.RestoredTs, 0);
}

/// Go uses `bytes.Replace(..., 1)`, so the public helper replaces the first
/// occurrence even when a caller passes a rule that is not a key prefix.
#[test]
fn test_rewrite_and_encode_raw_key_replaces_first_occurrence() {
    let rule = import_sstpb::RewriteRule {
        OldKeyPrefix: b"old".to_vec(),
        NewKeyPrefix: b"new".to_vec(),
        ..Default::default()
    };
    let encoded = RewriteAndEncodeRawKey(b"x-old-old", Some(&rule)).expect("encoded key");
    let (_, raw) = codec::DecodeBytes(&encoded, None).expect("decode key");
    assert_eq!(raw, b"x-new-old");
}

/// A nil protobuf rule has empty getter values in Go, so `bytes.Replace`
/// leaves the raw key unchanged and the helper still returns an encoded key.
#[test]
fn test_rewrite_and_encode_raw_key_without_rule_encodes_original() {
    let encoded = RewriteAndEncodeRawKey(b"unchanged", None).expect("encoded key");
    let (_, raw) = codec::DecodeBytes(&encoded, None).expect("decode key");
    assert_eq!(raw, b"unchanged");
}

// 构造带 handle 的行键：`t{id}_r` + EncodeInt(handle)。
fn encode_row_key_with_handle(table_id: i64, handle: i64) -> Vec<u8> {
    let mut key = tablecodec::GenTableRecordPrefix(table_id);
    // handle 使用 memcomparable 整数编码，保证有序。
    key.extend_from_slice(&codec::EncodeInt(None, handle));
    key
}

// 构造索引 seek 键：完整 index 前缀后追加已编码列值。
fn encode_index_seek_key(table_id: i64, index_id: i64, encoded_values: &[u8]) -> Vec<u8> {
    let mut key = tablecodec::EncodeTableIndexPrefix(table_id, index_id);
    key.extend_from_slice(encoded_values);
    key
}

/// TestValidateFileRewriteRule — empty range, no match, end-key missing, mismatch.
///
/// 校验 SST 文件起止键必须各自命中一致的重写规则；空键、无匹配、
/// 仅起点匹配、终点规则与起点重写目标不一致均应失败。
#[test]
fn test_validate_file_rewrite_rule() {
    // 仅注册表 1→2 的粗粒度前缀规则。
    let mut rules = RewriteRules {
        Data: vec![import_sstpb::RewriteRule {
            OldKeyPrefix: tablecodec::EncodeTablePrefix(1),
            NewKeyPrefix: tablecodec::EncodeTablePrefix(2),
            ..Default::default()
        }],
        ..Default::default()
    };

    // 空起止键：找不到任何规则。
    // 对应 Go：empty file range。
    let err = ValidateFileRewriteRule(
        &backuppb::File {
            Name: "file_write.sst".into(),
            StartKey: vec![],
            EndKey: vec![],
            ..Default::default()
        },
        Some(&rules),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("cannot find rewrite rule"),
        "{err}"
    );

    // 起止落在表 0/1：起点无规则。
    // 表 0 未注册，Find 失败优先于 mismatch。
    let err = ValidateFileRewriteRule(
        &backuppb::File {
            Name: "file_write.sst".into(),
            StartKey: tablecodec::EncodeTablePrefix(0),
            EndKey: tablecodec::EncodeTablePrefix(1),
            ..Default::default()
        },
        Some(&rules),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("cannot find rewrite rule"),
        "{err}"
    );

    // 起点表 1 可匹配，但终点表 2 尚无规则 → 仍报 cannot find。
    // 半开终点落在下一表前缀时常见此形态。
    let err = ValidateFileRewriteRule(
        &backuppb::File {
            Name: "file_write.sst".into(),
            StartKey: tablecodec::EncodeTablePrefix(1),
            EndKey: tablecodec::EncodeTablePrefix(2),
            ..Default::default()
        },
        Some(&rules),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("cannot find rewrite rule"),
        "{err}"
    );

    // 补上表 2→3 后两端都能匹配，但新前缀不一致 → mismatch。
    // 新目标 2 vs 3：校验「同一文件必须映到同一新前缀族」。
    rules.Data.push(import_sstpb::RewriteRule {
        OldKeyPrefix: tablecodec::EncodeTablePrefix(2),
        NewKeyPrefix: tablecodec::EncodeTablePrefix(3),
        ..Default::default()
    });
    let err = ValidateFileRewriteRule(
        &backuppb::File {
            Name: "file_write.sst".into(),
            StartKey: tablecodec::EncodeTablePrefix(1),
            EndKey: tablecodec::EncodeTablePrefix(2),
            ..Default::default()
        },
        Some(&rules),
    )
    .unwrap_err();
    assert!(err.to_string().contains("rewrite rule mismatch"), "{err}");

    // 终点规则把表 2 映回表 1，与起点 1→2 的目标仍不一致。
    // 交叉映射同样视为 mismatch，而非成功。
    rules.Data = vec![
        rules.Data[0].clone(),
        import_sstpb::RewriteRule {
            OldKeyPrefix: tablecodec::EncodeTablePrefix(2),
            NewKeyPrefix: tablecodec::EncodeTablePrefix(1),
            ..Default::default()
        },
    ];
    let err = ValidateFileRewriteRule(
        &backuppb::File {
            Name: "file_write.sst".into(),
            StartKey: tablecodec::EncodeTablePrefix(1),
            EndKey: tablecodec::EncodeTablePrefix(2),
            ..Default::default()
        },
        Some(&rules),
    )
    .unwrap_err();
    assert!(err.to_string().contains("rewrite rule mismatch"), "{err}");
}

/// TestRewriteFileKeys — raw SST vs encoded log keys, including table-id 767 edge.
///
/// 原始 SST 键经重写后再 EncodeBytes；日志键已是编码形态，走 GetRewriteEncodedKeys。
/// 表 ID 767→511：错误地走 Raw 路径时解码结果不得碰巧等于编码目标。
#[test]
fn test_rewrite_file_keys() {
    // 两套表前缀映射：1→2 与 767→511（边界 ID）。
    // 767 的编码形态易与邻近 ID 混淆，专测 Raw/Encoded 分流。
    let rewrite_rules = RewriteRules {
        Data: vec![
            import_sstpb::RewriteRule {
                NewKeyPrefix: tablecodec::GenTablePrefix(2),
                OldKeyPrefix: tablecodec::GenTablePrefix(1),
                ..Default::default()
            },
            import_sstpb::RewriteRule {
                NewKeyPrefix: tablecodec::GenTablePrefix(511),
                OldKeyPrefix: tablecodec::GenTablePrefix(767),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    // SST：未编码的行前缀半开区间。
    // End=PrefixNext(recordPrefix) 覆盖整张表行空间。
    let raw_key_file = backuppb::File {
        Name: "backup.sst".into(),
        StartKey: tablecodec::GenTableRecordPrefix(1),
        EndKey: tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(1)),
        ..Default::default()
    };
    let (start, end) = GetRewriteRawKeys(&raw_key_file, Some(&rewrite_rules)).unwrap();
    // Raw API 返回已 EncodeBytes 的键，需解码后再与表前缀比较。
    let (_, end) = codec::DecodeBytes(&end.unwrap(), None).unwrap();
    let (_, start) = codec::DecodeBytes(&start.unwrap(), None).unwrap();
    assert_eq!(start, tablecodec::GenTableRecordPrefix(2));
    assert_eq!(
        end,
        tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(2))
    );

    // 日志文件：键已是 EncodeBytes 形态（Go 用例拼写 bakcup 保留）。
    // 与 SST 对照：同语义前缀，不同编解码入口。
    let encode_key_file = backuppb::DataFileInfo {
        Path: "bakcup.log".into(),
        StartKey: codec::EncodeBytes(Vec::new(), &tablecodec::GenTableRecordPrefix(1)),
        EndKey: codec::EncodeBytes(
            Vec::new(),
            &tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(1)),
        ),
    };
    let (start, end) = GetRewriteEncodedKeys(&encode_key_file, Some(&rewrite_rules)).unwrap();
    // Encoded 路径输出仍保持编码字节，直接相等断言。
    // 不得再 DecodeBytes，否则与 Go 断言层级不一致。
    assert_eq!(
        start.unwrap(),
        codec::EncodeBytes(Vec::new(), &tablecodec::GenTableRecordPrefix(2))
    );
    assert_eq!(
        end.unwrap(),
        codec::EncodeBytes(
            Vec::new(),
            &tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(2))
        )
    );

    // 767 边界：输入已是编码键。
    let encode_key_file_767 = backuppb::DataFileInfo {
        Path: "bakcup.log".into(),
        StartKey: codec::EncodeBytes(Vec::new(), &tablecodec::GenTableRecordPrefix(767)),
        EndKey: codec::EncodeBytes(
            Vec::new(),
            &tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(767)),
        ),
    };
    // 误用 Raw API：不得碰巧得到正确的 511 编码结果。
    // 双重编码/错误解码会破坏前缀匹配，assert_ne 锁定该坑。
    let (start, end) = GetRewriteRawKeys(&encode_key_file_767, Some(&rewrite_rules)).unwrap();
    assert_ne!(
        start.unwrap(),
        codec::EncodeBytes(Vec::new(), &tablecodec::GenTableRecordPrefix(511))
    );
    assert_ne!(
        end.unwrap(),
        codec::EncodeBytes(
            Vec::new(),
            &tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(511))
        )
    );
    // 正确 Encoded API：767→511 前后缀均重写成功。
    // 与上一段 assert_ne 形成正反对照。
    let (start, end) = GetRewriteEncodedKeys(&encode_key_file_767, Some(&rewrite_rules)).unwrap();
    assert_eq!(
        start.unwrap(),
        codec::EncodeBytes(Vec::new(), &tablecodec::GenTableRecordPrefix(511))
    );
    assert_eq!(
        end.unwrap(),
        codec::EncodeBytes(
            Vec::new(),
            &tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(511))
        )
    );
}

/// TestRewriteRange — nil rules, both keys, end-only rule, table-id mismatch.
///
/// 表驱动：无规则原样返回；起止同前缀共同重写；仅终点命中时起点保留；
/// 起止表 ID 语义冲突时返回 ErrRestoreTableIDMismatch。
#[test]
fn test_rewrite_range() {
    // expected_error 与 expected_range 互斥：成功看 range，失败看错误串。
    struct Case {
        rg: Range,
        rewrite_rules: Option<RewriteRules>,
        expected_range: Option<Range>,
        expected_error: Option<SharedError>,
    }

    let cases = vec![
        // 规则为 None：区间字节完全不变。
        // 恢复早期尚未绑定表映射时的安全路径。
        Case {
            rg: Range {
                KeyRange: KeyRange {
                    StartKey: b"startKey".to_vec(),
                    EndKey: b"endKey".to_vec(),
                },
                Files: vec![],
            },
            rewrite_rules: None,
            expected_range: Some(Range {
                KeyRange: KeyRange {
                    StartKey: b"startKey".to_vec(),
                    EndKey: b"endKey".to_vec(),
                },
                Files: vec![],
            }),
            expected_error: None,
        },
        // 起止共享 index 前缀 1→2，后缀 startKey/endKey 保留。
        // 验证 RewriteRange 对两端应用同一前缀替换。
        Case {
            rg: Range {
                KeyRange: KeyRange {
                    StartKey: [tablecodec::GenTableIndexPrefix(1), b"startKey".to_vec()].concat(),
                    EndKey: [tablecodec::GenTableIndexPrefix(1), b"endKey".to_vec()].concat(),
                },
                Files: vec![],
            },
            rewrite_rules: Some(RewriteRules {
                Data: vec![import_sstpb::RewriteRule {
                    OldKeyPrefix: tablecodec::GenTableIndexPrefix(1),
                    NewKeyPrefix: tablecodec::GenTableIndexPrefix(2),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            expected_range: Some(Range {
                KeyRange: KeyRange {
                    StartKey: [tablecodec::GenTableIndexPrefix(2), b"startKey".to_vec()].concat(),
                    EndKey: [tablecodec::GenTableIndexPrefix(2), b"endKey".to_vec()].concat(),
                },
                Files: vec![],
            }),
            expected_error: None,
        },
        // 规则只覆盖完整 endKey：起点保持表 1，终点改写到 newEndKey。
        // 起点无匹配规则时允许不改写，与 Go 行为一致。
        Case {
            rg: Range {
                KeyRange: KeyRange {
                    StartKey: [tablecodec::GenTableIndexPrefix(1), b"startKey".to_vec()].concat(),
                    EndKey: [tablecodec::GenTableIndexPrefix(1), b"endKey".to_vec()].concat(),
                },
                Files: vec![],
            },
            rewrite_rules: Some(RewriteRules {
                Data: vec![import_sstpb::RewriteRule {
                    OldKeyPrefix: [tablecodec::GenTableIndexPrefix(1), b"endKey".to_vec()].concat(),
                    NewKeyPrefix: [tablecodec::GenTableIndexPrefix(2), b"newEndKey".to_vec()]
                        .concat(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            expected_range: Some(Range {
                KeyRange: KeyRange {
                    StartKey: [tablecodec::GenTableIndexPrefix(1), b"startKey".to_vec()].concat(),
                    EndKey: [tablecodec::GenTableIndexPrefix(2), b"newEndKey".to_vec()].concat(),
                },
                Files: vec![],
            }),
            expected_error: None,
        },
        // 人造 t1_/t2_ 前缀：起点重写后与终点表语义冲突。
        // 使用非标准键字节以触发 table id mismatch 分支。
        Case {
            rg: Range {
                KeyRange: KeyRange {
                    StartKey: b"t1_startKey".to_vec(),
                    EndKey: b"t2_endKey".to_vec(),
                },
                Files: vec![],
            },
            rewrite_rules: Some(RewriteRules {
                Data: vec![import_sstpb::RewriteRule {
                    OldKeyPrefix: b"t1_startKey".to_vec(),
                    NewKeyPrefix: b"t2_newStartKey".to_vec(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            expected_range: None,
            expected_error: Annotate(
                Some(SharedError::new((*ErrRestoreTableIDMismatch).clone())),
                "table id mismatch",
            ),
        },
    ];

    for tc in cases {
        let mut rg = tc.rg;
        // RewriteRange 就地改写 rg，并返回结果 Range。
        // as_ref 保持 Option 借用，避免移动 cases 内规则。
        let result = RewriteRange(&mut rg, tc.rewrite_rules.as_ref());
        match (tc.expected_error, result) {
            (Some(expected), Err(actual)) => {
                // 错误串需与 Annotate 包装后完全一致。
                // 同时要求失败用例不携带 expected_range。
                assert_eq!(expected.to_string(), actual.to_string());
                assert!(tc.expected_range.is_none());
            }
            (None, Ok(actual_range)) => {
                assert_eq!(tc.expected_range.unwrap(), actual_range);
            }
            (expected, actual) => {
                // Ok/Err 形态与期望交叉时直接 panic，避免静默通过。
                panic!("unexpected result: expected={expected:?} actual={actual:?}")
            }
        }
    }
}

/// TestGetRewriteTableID — table prefix and record prefix both resolve new id.
///
/// 粗粒度表前缀与行记录前缀都应能从旧 ID 解析出新表 ID。
#[test]
fn test_get_rewrite_table_id() {
    let table_id: i64 = 76;
    let old_table_id: i64 = 80;
    {
        // EncodeTablePrefix(old)→EncodeTablePrefix(new)。
        // 80→76：任意正 ID 均可，重点在解析路径。
        let rewrite_rules = RewriteRules {
            Data: vec![import_sstpb::RewriteRule {
                OldKeyPrefix: tablecodec::EncodeTablePrefix(old_table_id),
                NewKeyPrefix: tablecodec::EncodeTablePrefix(table_id),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(GetRewriteTableID(old_table_id, &rewrite_rules), table_id);
    }
    {
        // 仅有 record 前缀规则时同样可解析表 ID。
        // 细粒度备份只写 _r 前缀时仍需 GetRewriteTableID 可用。
        let rewrite_rules = RewriteRules {
            Data: vec![import_sstpb::RewriteRule {
                OldKeyPrefix: tablecodec::GenTableRecordPrefix(old_table_id),
                NewKeyPrefix: tablecodec::GenTableRecordPrefix(table_id),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(GetRewriteTableID(old_table_id, &rewrite_rules), table_id);
    }
}

// 测试辅助：按 starts_with 取首条匹配规则的新前缀。
// 顺序敏感：与生产 Find 一样依赖规则列表次序。
fn get_new_key_prefix(key: &[u8], rewrite_rules: &RewriteRules) -> Option<Vec<u8>> {
    for rule in &rewrite_rules.Data {
        // 最长前缀策略未实现；本测数据保证无歧义重叠。
        if key.starts_with(&rule.GetOldKeyPrefix()) {
            return Some(rule.GetNewKeyPrefix());
        }
    }
    None
}

/// 构造新旧 TableInfo：表 2→1，分区 101/201→100/200，索引名 i1/i2 对齐。
fn generate_rewrite_table_infos() -> (model::TableInfo, model::TableInfo) {
    // 新表：主表 ID=1，分区 100/200，索引 1/2。
    // 分区名 p1/p2 与旧表对齐，供按名匹配。
    let new_table_info = model::TableInfo {
        ID: 1,
        Indices: vec![
            model::IndexInfo {
                ID: 1,
                Name: model::CIStr::new("i1"),
            },
            model::IndexInfo {
                ID: 2,
                Name: model::CIStr::new("i2"),
            },
        ],
        Partition: Some(model::PartitionInfo {
            Definitions: vec![
                model::PartitionDefinition {
                    ID: 100,
                    Name: model::CIStr::new("p1"),
                },
                model::PartitionDefinition {
                    ID: 200,
                    Name: model::CIStr::new("p2"),
                },
            ],
        }),
    };
    // 旧表：主表 ID=2，分区 101/201；索引名相同以便按名重映射。
    // 分区物理 ID 刻意不同于新表，验证 ID 重写而非名重写。
    let old_table_info = model::TableInfo {
        ID: 2,
        Indices: vec![
            model::IndexInfo {
                ID: 1,
                Name: model::CIStr::new("i1"),
            },
            model::IndexInfo {
                ID: 2,
                Name: model::CIStr::new("i2"),
            },
        ],
        Partition: Some(model::PartitionInfo {
            Definitions: vec![
                model::PartitionDefinition {
                    ID: 101,
                    Name: model::CIStr::new("p1"),
                },
                model::PartitionDefinition {
                    ID: 201,
                    Name: model::CIStr::new("p2"),
                },
            ],
        }),
    };
    (new_table_info, old_table_info)
}

/// TestGetRewriteRules — coarse table prefix and detailed record/index rules.
///
/// `newDataFile=false` 只生成表级前缀；`true` 展开 record/index 细规则，
/// 含主表与各分区上的索引 ID 映射。
#[test]
fn test_get_rewrite_rules() {
    let (new_table_info, old_table_info) = generate_rewrite_table_infos();
    {
        // 粗粒度：主表 2→1，分区 101→100、201→200。
        // 第三参 newTS=0：本测不关心时间戳填充。
        let rewrite_rules = GetRewriteRules(&new_table_info, &old_table_info, 0, false);
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTablePrefix(2), &rewrite_rules),
            Some(tablecodec::EncodeTablePrefix(1))
        );
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTablePrefix(101), &rewrite_rules),
            Some(tablecodec::EncodeTablePrefix(100))
        );
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTablePrefix(201), &rewrite_rules),
            Some(tablecodec::EncodeTablePrefix(200))
        );
    }
    {
        // 细粒度：校验主表与两分区的 record 及 index1/index2 前缀。
        // true=newDataFile：展开 _r/_i 规则。
        let rewrite_rules = GetRewriteRules(&new_table_info, &old_table_info, 0, true);
        // 主表行前缀 2→1。
        // 细粒度下不再暴露裸 EncodeTablePrefix 匹配（由 record/index 覆盖）。
        assert_eq!(
            get_new_key_prefix(&tablecodec::GenTableRecordPrefix(2), &rewrite_rules),
            Some(tablecodec::GenTableRecordPrefix(1))
        );
        // 主表索引 1、2 保持 index ID，仅换表 ID。
        // 与 GetRewriteRuleOfTable 中显式 1→10 映射场景区分。
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTableIndexPrefix(2, 1), &rewrite_rules),
            Some(tablecodec::EncodeTableIndexPrefix(1, 1))
        );
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTableIndexPrefix(2, 2), &rewrite_rules),
            Some(tablecodec::EncodeTableIndexPrefix(1, 2))
        );
        // 分区 p1：101→100。
        // 分区视为独立物理表 ID 参与前缀重写。
        assert_eq!(
            get_new_key_prefix(&tablecodec::GenTableRecordPrefix(101), &rewrite_rules),
            Some(tablecodec::GenTableRecordPrefix(100))
        );
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTableIndexPrefix(101, 1), &rewrite_rules),
            Some(tablecodec::EncodeTableIndexPrefix(100, 1))
        );
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTableIndexPrefix(101, 2), &rewrite_rules),
            Some(tablecodec::EncodeTableIndexPrefix(100, 2))
        );
        // 分区 p2：201→200。
        // 与 p1 对称，防止只测到第一个分区。
        assert_eq!(
            get_new_key_prefix(&tablecodec::GenTableRecordPrefix(201), &rewrite_rules),
            Some(tablecodec::GenTableRecordPrefix(200))
        );
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTableIndexPrefix(201, 1), &rewrite_rules),
            Some(tablecodec::EncodeTableIndexPrefix(200, 1))
        );
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTableIndexPrefix(201, 2), &rewrite_rules),
            Some(tablecodec::EncodeTableIndexPrefix(200, 2))
        );
    }
}

/// TestGetRewriteRulesMap — rules bucketed by old table/partition id.
///
/// 与 GetRewriteRules 语义相同，但按旧表/分区 ID 分桶，便于按物理 ID 查找。
#[test]
fn test_get_rewrite_rules_map() {
    let (new_table_info, old_table_info) = generate_rewrite_table_infos();
    {
        // 粗粒度 Map：键为旧主表/分区 ID。
        // 调用方按文件物理 ID 取桶，避免扫描全量规则。
        let rewrite_rules = GetRewriteRulesMap(&new_table_info, &old_table_info, 0, false);
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTablePrefix(2), &rewrite_rules[&2]),
            Some(tablecodec::EncodeTablePrefix(1))
        );
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTablePrefix(101), &rewrite_rules[&101]),
            Some(tablecodec::EncodeTablePrefix(100))
        );
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTablePrefix(201), &rewrite_rules[&201]),
            Some(tablecodec::EncodeTablePrefix(200))
        );
    }
    {
        // 细粒度：每个桶内含该物理表的 record/index 规则。
        // 桶之间规则不共享，避免跨分区误匹配。
        let rewrite_rules = GetRewriteRulesMap(&new_table_info, &old_table_info, 0, true);
        // 桶 2：主表细规则。
        // 使用 map 下标 &2，缺失即 panic，暴露漏桶。
        assert_eq!(
            get_new_key_prefix(&tablecodec::GenTableRecordPrefix(2), &rewrite_rules[&2]),
            Some(tablecodec::GenTableRecordPrefix(1))
        );
        assert_eq!(
            get_new_key_prefix(
                &tablecodec::EncodeTableIndexPrefix(2, 1),
                &rewrite_rules[&2]
            ),
            Some(tablecodec::EncodeTableIndexPrefix(1, 1))
        );
        assert_eq!(
            get_new_key_prefix(
                &tablecodec::EncodeTableIndexPrefix(2, 2),
                &rewrite_rules[&2]
            ),
            Some(tablecodec::EncodeTableIndexPrefix(1, 2))
        );
        // 桶 101：分区 p1。
        // 索引断言紧随其后，确保桶内同时含 _i 规则。
        assert_eq!(
            get_new_key_prefix(&tablecodec::GenTableRecordPrefix(101), &rewrite_rules[&101]),
            Some(tablecodec::GenTableRecordPrefix(100))
        );
        assert_eq!(
            get_new_key_prefix(
                &tablecodec::EncodeTableIndexPrefix(101, 1),
                &rewrite_rules[&101]
            ),
            Some(tablecodec::EncodeTableIndexPrefix(100, 1))
        );
        assert_eq!(
            get_new_key_prefix(
                &tablecodec::EncodeTableIndexPrefix(101, 2),
                &rewrite_rules[&101]
            ),
            Some(tablecodec::EncodeTableIndexPrefix(100, 2))
        );
        // 桶 201：分区 p2。
        // 与列表版 GetRewriteRules 结果逐键对齐。
        assert_eq!(
            get_new_key_prefix(&tablecodec::GenTableRecordPrefix(201), &rewrite_rules[&201]),
            Some(tablecodec::GenTableRecordPrefix(200))
        );
        assert_eq!(
            get_new_key_prefix(
                &tablecodec::EncodeTableIndexPrefix(201, 1),
                &rewrite_rules[&201]
            ),
            Some(tablecodec::EncodeTableIndexPrefix(200, 1))
        );
        assert_eq!(
            get_new_key_prefix(
                &tablecodec::EncodeTableIndexPrefix(201, 2),
                &rewrite_rules[&201]
            ),
            Some(tablecodec::EncodeTableIndexPrefix(200, 2))
        );
    }
}

/// TestGetRewriteRuleOfTable — coarse/detail rules, TS range, empty index map.
///
/// 直接按旧/新表 ID 与索引映射生成规则；覆盖粗/细粒度、TS 字段与空索引映射。
#[test]
fn test_get_rewrite_rule_of_table() {
    {
        // 粗粒度：仅一条表前缀，并填充 RemapHint/NewTableID。
        // 传入的索引 map 在 false 模式下不应增加 Data 条数。
        let rewrite_rules = GetRewriteRuleOfTable(2, 1, HashMap::from([(1, 1), (2, 2)]), false);
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTablePrefix(2), &rewrite_rules),
            Some(tablecodec::EncodeTablePrefix(1))
        );
        // Data 仅 1 条粗规则（索引映射在粗模式被忽略）。
        // NewTableID 供下游日志/校验快捷读取。
        assert_eq!(rewrite_rules.Data.len(), 1);
        assert_eq!(rewrite_rules.NewTableID, 1);
        assert_eq!(
            rewrite_rules.TableIDRemapHint,
            vec![TableIDRemap {
                Origin: 2,
                Rewritten: 1
            }]
        );
    }
    {
        // 细粒度：record + 两个 index → Data.len()==3。
        // 索引 ID 原样映射 (1,1)/(2,2)。
        let rewrite_rules = GetRewriteRuleOfTable(2, 1, HashMap::from([(1, 1), (2, 2)]), true);
        assert_eq!(
            get_new_key_prefix(&tablecodec::GenTableRecordPrefix(2), &rewrite_rules),
            Some(tablecodec::GenTableRecordPrefix(1))
        );
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTableIndexPrefix(2, 1), &rewrite_rules),
            Some(tablecodec::EncodeTableIndexPrefix(1, 1))
        );
        assert_eq!(
            get_new_key_prefix(&tablecodec::EncodeTableIndexPrefix(2, 2), &rewrite_rules),
            Some(tablecodec::EncodeTableIndexPrefix(1, 2))
        );
        assert_eq!(rewrite_rules.Data.len(), 3);
    }
    {
        // SetTsRange 写入 Shift/Start/Restored，不影响已有 RemapHint。
        // 供后续 SetTimeRangeFilter 读取。
        let shift_start_ts = 30u64;
        let start_ts = 50u64;
        let restored_ts = 100u64;
        let mut rewrite_rules = GetRewriteRuleOfTable(2, 1, HashMap::from([(1, 1)]), true);
        rewrite_rules.SetTsRange(shift_start_ts, start_ts, restored_ts);
        assert_eq!(rewrite_rules.RestoredTs, restored_ts);
        assert_eq!(rewrite_rules.StartTs, start_ts);
        assert_eq!(rewrite_rules.ShiftStartTs, shift_start_ts);
        assert_eq!(
            rewrite_rules.TableIDRemapHint,
            vec![TableIDRemap {
                Origin: 2,
                Rewritten: 1
            }]
        );
        assert_eq!(rewrite_rules.NewTableID, 1);
    }
    {
        // 空索引映射：细粒度仍至少有 record 规则一条。
        // 无二级索引的表恢复路径。
        let rewrite_rules = GetRewriteRuleOfTable(2, 1, HashMap::new(), true);
        assert_eq!(rewrite_rules.Data.len(), 1);
        assert_eq!(
            get_new_key_prefix(&tablecodec::GenTableRecordPrefix(2), &rewrite_rules),
            Some(tablecodec::GenTableRecordPrefix(1))
        );
    }
}

/// 测试替身 AppliedFile：显式指定起止键字节。
/// 避免构造完整 backuppb.File 字段。
struct FakeApplyFile {
    start_key: Vec<u8>,
    end_key: Vec<u8>,
}

impl AppliedFile for FakeApplyFile {
    // 返回克隆，满足 getter 值语义。
    fn GetStartKey(&self) -> Vec<u8> {
        self.start_key.clone()
    }
    // 与 StartKey 成对提供半开上界。
    fn GetEndKey(&self) -> Vec<u8> {
        self.end_key.clone()
    }
}

/// Mirrors Go rewriteKey helper (uses NewKeyPrefix len for suffix — same when lens equal).
///
/// 注意：后缀切片使用 NewKeyPrefix 长度——当前用例旧/新前缀等长时与 Go 一致。
fn rewrite_key(key: &[u8], rule: &import_sstpb::RewriteRule) -> Option<Vec<u8>> {
    if key.starts_with(&rule.GetOldKeyPrefix()) {
        // 新前缀 + 原键去掉「新前缀长度」后的后缀（等长前提）。
        Some(
            [
                &rule.GetNewKeyPrefix()[..],
                &key[rule.GetNewKeyPrefix().len()..],
            ]
            .concat(),
        )
    } else {
        None
    }
}

/// TestFindMatchedRewriteRule — row key, index key, cross-table, unmatched prefix.
///
/// 同行/同索引应命中规则；跨表起止或未注册前缀返回 None。
#[test]
fn test_find_matched_rewrite_rule() {
    // 索引映射 1→10：验证 index 前缀重写目标 index ID。
    // 与同 ID 映射用例区分，防止只测恒等映射。
    let rewrite_rules = GetRewriteRuleOfTable(2, 1, HashMap::from([(1, 10)]), true);
    {
        // 同行键 [handle 100,200)：匹配 record 规则，handle 不变。
        // handle 落在后缀，重写后仍为 100。
        let apply_file = FakeApplyFile {
            start_key: encode_row_key_with_handle(2, 100),
            end_key: encode_row_key_with_handle(2, 200),
        };
        let rule = FindMatchedRewriteRule(&apply_file, &rewrite_rules).expect("rule");
        assert_eq!(
            rewrite_key(&encode_row_key_with_handle(2, 100), &rule),
            Some(encode_row_key_with_handle(1, 100))
        );
    }
    {
        // 索引键：旧 index=1 → 新 index=10，列值后缀保留。
        // encoded_values 使用明文 "test-*"，足够区分起止。
        let apply_file = FakeApplyFile {
            start_key: encode_index_seek_key(2, 1, b"test-1"),
            end_key: encode_index_seek_key(2, 1, b"test-2"),
        };
        let rule = FindMatchedRewriteRule(&apply_file, &rewrite_rules).expect("rule");
        assert_eq!(
            rewrite_key(&encode_index_seek_key(2, 1, b"test-1"), &rule),
            Some(encode_index_seek_key(1, 10, b"test-1"))
        );
    }
    {
        // 起止跨表 1 与 2：无法唯一匹配。
        // 生产中跨表 SST 本就不合法，此处锁定返回 None。
        let apply_file = FakeApplyFile {
            start_key: encode_row_key_with_handle(1, 100),
            end_key: encode_row_key_with_handle(2, 200),
        };
        assert!(FindMatchedRewriteRule(&apply_file, &rewrite_rules).is_none());
    }
    {
        // 前缀落在新表 1：规则只描述旧表 2，故无匹配。
        // 防止「已重写键」再次被旧规则命中。
        let apply_file = FakeApplyFile {
            start_key: tablecodec::EncodeTablePrefix(1),
            end_key: tablecodec::EncodeTablePrefix(1),
        };
        assert!(FindMatchedRewriteRule(&apply_file, &rewrite_rules).is_none());
    }
}

/// TestGetRewriteKeyWithDifferentTable — cross-table start/end errors for raw & encoded.
///
/// 无规则且起止属不同表时，Raw/Encoded 两条路径均应报错。
#[test]
fn test_get_rewrite_key_with_different_table() {
    // 起止表 ID 分别为 1 与 2。
    let apply_file = FakeApplyFile {
        start_key: encode_row_key_with_handle(1, 100),
        end_key: encode_row_key_with_handle(2, 200),
    };
    // rules=None：仍因跨表键失败。
    // Raw/Encoded 错误路径应对称存在。
    assert!(GetRewriteRawKeys(&apply_file, None).is_err());
    assert!(GetRewriteEncodedKeys(&apply_file, None).is_err());
}

/// TestSetTimeRangeFilter — default/write CF filters and invalid CF error.
///
/// default CF 在 ShiftStartTs<StartTs 时用 Shift 作为 IgnoreBefore；
/// write CF 固定用 StartTs；非法 CF 名报错；零时间戳跳过过滤。
#[test]
fn test_set_time_range_filter() {
    // name 仅用于断言失败信息，便于对照 Go 子用例。
    // expect_error=true 时不检查 Ignore* 字段。
    struct Case {
        name: &'static str,
        rules: RewriteRules,
        cf_name: &'static str,
        expect_error: bool,
    }

    let test_cases = vec![
        // default：IgnoreBefore=Shift(50)，IgnoreAfter=Restored(200)。
        // 覆盖最常见的 default CF 过滤配置。
        Case {
            name: "default cf with valid timestamps",
            rules: RewriteRules {
                Data: vec![import_sstpb::RewriteRule {
                    OldKeyPrefix: b"old".to_vec(),
                    NewKeyPrefix: b"new".to_vec(),
                    ..Default::default()
                }],
                ShiftStartTs: 50,
                StartTs: 100,
                RestoredTs: 200,
                ..Default::default()
            },
            cf_name: "default",
            expect_error: false,
        },
        // write：IgnoreBefore 取 StartTs(100)，忽略 Shift。
        // write CF 语义更窄，避免前移到 Shift。
        Case {
            name: "write cf with valid timestamps",
            rules: RewriteRules {
                Data: vec![import_sstpb::RewriteRule {
                    OldKeyPrefix: b"old".to_vec(),
                    NewKeyPrefix: b"new".to_vec(),
                    ..Default::default()
                }],
                ShiftStartTs: 50,
                StartTs: 100,
                RestoredTs: 200,
                ..Default::default()
            },
            cf_name: "write",
            expect_error: false,
        },
        // Shift>Start：default 回退用 StartTs 作为 IgnoreBefore。
        // 名称含 invalid，但 expect_error=false：仅 Shift 无效非 CF 错误。
        Case {
            name: "invalid shift start ts (greater than start ts)",
            rules: RewriteRules {
                Data: vec![import_sstpb::RewriteRule {
                    OldKeyPrefix: b"old".to_vec(),
                    NewKeyPrefix: b"new".to_vec(),
                    ..Default::default()
                }],
                ShiftStartTs: 150,
                StartTs: 100,
                RestoredTs: 200,
                ..Default::default()
            },
            cf_name: "default",
            expect_error: false,
        },
        // write 在 Shift>Start 时仍只用 StartTs。
        // 与 default 回退分支对照。
        Case {
            name: "write cf valid shift start ts (greater than start ts)",
            rules: RewriteRules {
                Data: vec![import_sstpb::RewriteRule {
                    OldKeyPrefix: b"old".to_vec(),
                    NewKeyPrefix: b"new".to_vec(),
                    ..Default::default()
                }],
                ShiftStartTs: 150,
                StartTs: 100,
                RestoredTs: 200,
                ..Default::default()
            },
            cf_name: "write",
            expect_error: false,
        },
        // 未知 CF：期望错误。
        // 唯一 expect_error=true 的用例。
        Case {
            name: "invalid cf name",
            rules: RewriteRules {
                Data: vec![import_sstpb::RewriteRule {
                    OldKeyPrefix: b"old".to_vec(),
                    NewKeyPrefix: b"new".to_vec(),
                    ..Default::default()
                }],
                ShiftStartTs: 50,
                StartTs: 100,
                RestoredTs: 200,
                ..Default::default()
            },
            cf_name: "invalid",
            expect_error: true,
        },
        // 零时间戳：不写入 Ignore* 过滤字段。
        // 断言检查的是 rules.Data 原值，而非输出 rule。
        Case {
            name: "zero timestamps should skip filter",
            rules: RewriteRules {
                Data: vec![import_sstpb::RewriteRule {
                    OldKeyPrefix: b"old".to_vec(),
                    NewKeyPrefix: b"new".to_vec(),
                    ..Default::default()
                }],
                StartTs: 0,
                RestoredTs: 0,
                ShiftStartTs: 0,
                ..Default::default()
            },
            cf_name: "default",
            expect_error: false,
        },
    ];

    for tc in test_cases {
        // 每次用干净 rule 承接过滤器写入。
        // 避免用例间字段残留。
        let mut rule = import_sstpb::RewriteRule::default();
        let err = SetTimeRangeFilter(&tc.rules, &mut rule, tc.cf_name);
        if tc.expect_error {
            assert!(err.is_err(), "{}", tc.name);
            continue;
        }
        assert!(err.is_ok(), "{}: {err:?}", tc.name);

        // 零 TS：确认规则 Data 内 Ignore* 仍为 0（未启用过滤）。
        // Start 或 Restored 任一为 0 即跳过。
        if tc.rules.StartTs == 0 || tc.rules.RestoredTs == 0 {
            for r in &tc.rules.Data {
                assert_eq!(r.IgnoreBeforeTimestamp, 0, "{}", tc.name);
                assert_eq!(r.IgnoreAfterTimestamp, 0, "{}", tc.name);
            }
            continue;
        }

        // IgnoreAfter 恒为 RestoredTs。
        // default/write 在上界上行为相同。
        assert_eq!(
            rule.IgnoreAfterTimestamp, tc.rules.RestoredTs,
            "{}",
            tc.name
        );
        if tc.cf_name.contains("default") {
            if tc.rules.ShiftStartTs < tc.rules.StartTs {
                // Shift 更早：default 用 Shift 扩宽下界。
                // 以包含 shift 窗口内的 default CF 版本。
                assert_eq!(
                    rule.IgnoreBeforeTimestamp, tc.rules.ShiftStartTs,
                    "{}",
                    tc.name
                );
            } else {
                // Shift 无效时退回 StartTs。
                // 防止 IgnoreBefore > Start 造成空过滤窗口。
                assert_eq!(rule.IgnoreBeforeTimestamp, tc.rules.StartTs, "{}", tc.name);
            }
        } else if tc.cf_name.contains("write") {
            // write CF 从不采用 ShiftStartTs。
            // contains("write") 与 Go 字符串判断保持一致。
            assert_eq!(rule.IgnoreBeforeTimestamp, tc.rules.StartTs, "{}", tc.name);
        }
    }
}

/// TestSetTimeRangeFilterRace — 100 threads read shared rules, each owns its rule.
///
/// 对齐 Go 竞态用例：共享 rules 只读克隆进各线程，每线程私有 rule 输出；
/// 聚合后 IgnoreBefore/After 均应为 50/200，且主线程 rules TS 未被改写。
#[test]
fn test_set_time_range_filter_race() {
    // 共享源规则：default CF 期望 IgnoreBefore=Shift=50。
    // RestoredTs=200 作为 IgnoreAfter 金标准。
    let rules = RewriteRules {
        Data: vec![import_sstpb::RewriteRule {
            OldKeyPrefix: b"old".to_vec(),
            NewKeyPrefix: b"new".to_vec(),
            ..Default::default()
        }],
        ShiftStartTs: 50,
        StartTs: 100,
        RestoredTs: 200,
        ..Default::default()
    };
    // 命名保留 goroutines，对应 Go 并发度 100。
    // channel 收集每线程写出的 rule。
    let num_goroutines = 100;
    let (tx, rx) = mpsc::channel();

    for _ in 0..num_goroutines {
        let tx = tx.clone();
        // clone rules：模拟多读者，避免跨线程共享可变引用。
        let rules = rules.clone();
        thread::spawn(move || {
            // 线程局部 rule：写入端无共享。
            let mut rule = import_sstpb::RewriteRule::default();
            let err = SetTimeRangeFilter(&rules, &mut rule, "default");
            if err.is_err() {
                // 失败用 None 占位，主线程 expect 会暴露。
                let _ = tx.send(None);
                return;
            }
            // 成功则移出 rule 供主线程校验字段。
            let _ = tx.send(Some(rule));
        });
    }
    // 放下发送端，使 recv 能在全部完成后结束。
    drop(tx);

    for _ in 0..num_goroutines {
        // 任一线程失败会在 expect("rule") 处暴露。
        let rule = rx.recv().expect("result").expect("rule");
        // 每线程独立写出的过滤字段必须一致。
        assert_eq!(rule.IgnoreBeforeTimestamp, 50);
        assert_eq!(rule.IgnoreAfterTimestamp, 200);
    }

    // 源 rules 时间戳未被并发路径篡改。
    // 证明 SetTimeRangeFilter 未写回共享 rules。
    assert_eq!(rules.ShiftStartTs, 50);
    assert_eq!(rules.StartTs, 100);
    // RestoredTs 同样应保持初值 200。
    assert_eq!(rules.RestoredTs, 200);
}
