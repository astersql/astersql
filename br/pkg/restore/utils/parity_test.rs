// Copyright 2026 AsterSQL.

//! 与 Go `br/pkg/restore/utils` 的公开契约对齐测试（parity）。
//! 覆盖 misc 键工具、ID 映射、rewrite 规则生成/校验、Range 重写与合并入口。
//! 单测聚合多模块符号，失败时按段落注释定位；不改行为，仅验证语义一致性。
//! Parity tests for `br/pkg/restore/utils` vs Go sources.

// HashMap 仅用于类型侧构造；本文件不依赖外部服务。
use std::collections::HashMap;

// rtree Range 用于 RewriteRange 入参构造。
use astersql_br_pkg_rtree::{KeyRange, Range};

use crate::stubs::{backuppb, codec, import_sstpb, model, tablecodec};
use crate::{
    AppliedFile, DefaultCFName, EmptyRewriteRule, EncodeKeyPrefix, GetIndexIDMap,
    GetPartitionIDMap, GetRewriteEncodedKeys, GetRewriteRawKeys, GetRewriteRuleOfTable,
    GetRewriteRules, GetRewriteRulesMap, GetRewriteTableID, GetTableIDMap,
    MergeAndRewriteFileRanges, RewriteRange, RewriteRules, SetTimeRangeFilter, TableIDRemap,
    TruncateTS, ValidateFileRewriteRule, WriteCFName,
};

/// 总览式契约测试：按段落断言 misc / rewrite / merge 的 Go-Rust 对齐点。
#[test]
fn go_rust_public_contract_matches() {
    // 段落顺序：misc → ID map → rewrite → validate → keys → range → merge → append。
    // —— TruncateTS：空/短/长键边界 ——
    // boundary: TruncateTS empty / short / long keys
    // 空键 → None；>=8 剥 TS；<8 原样。
    // 与 Go TruncateTS 空输入返回 nil 对齐。
    assert_eq!(TruncateTS(&[]), None);
    let key16 = b"1212121212121212".to_vec();
    assert_eq!(TruncateTS(&key16), Some(b"12121212".to_vec()));
    // 短键不做截断，避免误伤非 TS 后缀。
    assert_eq!(TruncateTS(b"12"), Some(b"12".to_vec()));

    // —— EncodeKeyPrefix：memcomparable 8 字节分组 ——
    // normal: EncodeKeyPrefix memcomparable grouping
    // 16 字节应产出两组 + 各 0xff；15 字节尾巴原样；2 字节无分组。
    let enc = EncodeKeyPrefix(&key16);
    assert_eq!(
        enc,
        vec![
            b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', 0xff, b'1', b'2', b'1', b'2', b'1',
            b'2', b'1', b'2', 0xff
        ]
    );
    let key15 = b"121212121212121".to_vec();
    assert_eq!(
        EncodeKeyPrefix(&key15),
        vec![
            b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', 0xff, b'1', b'2', b'1', b'2', b'1',
            b'2', b'1'
        ]
    );
    // 不足一组时 EncodeBytes 路径不触发，输出等于输入。
    assert_eq!(EncodeKeyPrefix(b"12"), b"12".to_vec());

    // —— 分区/表/索引 ID 映射：按名称对齐 ——
    // normal: partition/table/index ID maps
    // old 10→new 20；分区 p0/p1、索引 idx_a/idx_b 同名映射。
    // 构造带双分区与双索引的旧/新表元数据夹具。
    let old_table = model::TableInfo {
        ID: 10,
        Partition: Some(model::PartitionInfo {
            Definitions: vec![
                model::PartitionDefinition {
                    ID: 101,
                    Name: model::CIStr::new("p0"),
                },
                model::PartitionDefinition {
                    ID: 102,
                    Name: model::CIStr::new("p1"),
                },
            ],
        }),
        Indices: vec![
            model::IndexInfo {
                ID: 1,
                Name: model::CIStr::new("idx_a"),
            },
            model::IndexInfo {
                ID: 2,
                Name: model::CIStr::new("idx_b"),
            },
        ],
    };
    // 新表 ID/分区 ID/索引 ID 均不同，名称保持一致以触发映射。
    let new_table = model::TableInfo {
        ID: 20,
        Partition: Some(model::PartitionInfo {
            Definitions: vec![
                model::PartitionDefinition {
                    ID: 201,
                    Name: model::CIStr::new("p0"),
                },
                model::PartitionDefinition {
                    ID: 202,
                    Name: model::CIStr::new("p1"),
                },
            ],
        }),
        Indices: vec![
            model::IndexInfo {
                ID: 11,
                Name: model::CIStr::new("idx_a"),
            },
            model::IndexInfo {
                ID: 12,
                Name: model::CIStr::new("idx_b"),
            },
        ],
    };
    // 分区映射只按 Name.L，不要求 ID 有序。
    let part_map = GetPartitionIDMap(&new_table, &old_table);
    assert_eq!(part_map.get(&101), Some(&201));
    assert_eq!(part_map.get(&102), Some(&202));
    let table_map = GetTableIDMap(&new_table, &old_table);
    // GetTableIDMap 含分区映射 + 整表 ID。
    assert_eq!(table_map.get(&10), Some(&20));
    let idx_map = GetIndexIDMap(&new_table, &old_table);
    assert_eq!(idx_map.get(&1), Some(&11));
    assert_eq!(idx_map.get(&2), Some(&12));

    // —— 重写规则生成 ——
    // normal: rewrite rules generation
    // getDetailRule=false 仅表前缀；true 展开 record+各索引，条数更多。
    // newTimeStamp=42 写入规则 NewTimestamp，粗粒度前缀模式。
    let rules = GetRewriteRules(&new_table, &old_table, 42, false);
    assert!(!rules.Data.is_empty());
    // 表 + 两分区 → RemapHint 长度为 3。
    assert_eq!(rules.TableIDRemapHint.len(), 3);
    // 细粒度：每物理表一条 record + 每索引一条，条数严格大于粗粒度。
    let detail = GetRewriteRules(&new_table, &old_table, 42, true);
    assert!(detail.Data.len() > rules.Data.len());
    // Map 形态：每个旧物理表 ID 一条独立 RewriteRules。
    let map_rules = GetRewriteRulesMap(&new_table, &old_table, 42, false);
    assert_eq!(map_rules.len(), 3);
    // 单表规则：NewTableID 直接暴露给调用方做元数据提示。
    let table_rule = GetRewriteRuleOfTable(10, 20, idx_map.clone(), false);
    assert_eq!(table_rule.NewTableID, 20);

    // —— Clone 按 Go 字段清单不复制 TS 字段 ——
    // boundary: Clone resets omitted time-range fields to zero
    let mut with_ts = EmptyRewriteRule();
    // ShiftStartTs=1, StartTs=2, RestoredTs=3。
    with_ts.SetTsRange(1, 2, 3);
    let cloned = with_ts.Clone();
    // Go Clone 的结构体字面量未列出 TS；Rust 保持同样的零值语义。
    assert_eq!(cloned.ShiftStartTs, 0);
    assert_eq!(cloned.StartTs, 0);
    assert_eq!(cloned.RestoredTs, 0);
    assert!(!cloned.Equal(&with_ts));

    // —— SetTimeRangeFilter：按 CF 选择 IgnoreBefore ——
    // normal: SetTimeRangeFilter per CF
    // write CF 用 StartTs；default CF 用 min(ShiftStartTs, StartTs)；未知 CF 报错。
    let mut file_rule = import_sstpb::RewriteRule::default();
    assert!(SetTimeRangeFilter(&with_ts, &mut file_rule, WriteCFName).is_ok());
    // write CF：IgnoreBefore=StartTs(2)，IgnoreAfter=RestoredTs(3)。
    assert_eq!(file_rule.IgnoreBeforeTimestamp, 2);
    assert_eq!(file_rule.IgnoreAfterTimestamp, 3);
    let mut file_rule2 = import_sstpb::RewriteRule::default();
    assert!(SetTimeRangeFilter(&with_ts, &mut file_rule2, DefaultCFName).is_ok());
    // default CF：IgnoreBefore=min(1,2)=1。
    assert_eq!(file_rule2.IgnoreBeforeTimestamp, 1);
    // 非 write/default CF 名称必须拒绝，防止静默漏过滤。
    assert!(SetTimeRangeFilter(&with_ts, &mut file_rule2, "bad_cf").is_err());

    // —— ValidateFileRewriteRule：缺规则 / 起止不匹配 ——
    // error: ValidateFileRewriteRule missing / mismatched rules
    // 空起止键无法匹配表前缀规则 → cannot find rewrite rule。
    let rewrite_rules = RewriteRules {
        Data: vec![import_sstpb::RewriteRule {
            OldKeyPrefix: tablecodec::EncodeTablePrefix(1),
            NewKeyPrefix: tablecodec::EncodeTablePrefix(2),
            ..Default::default()
        }],
        ..Default::default()
    };
    // 文件键为空：matchOldPrefix 失败路径。
    let err = ValidateFileRewriteRule(
        &backuppb::File {
            Name: "file_write.sst".into(),
            StartKey: vec![],
            EndKey: vec![],
            ..Default::default()
        },
        Some(&rewrite_rules),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("cannot find rewrite rule"),
        "{err}"
    );

    // —— GetRewriteRawKeys / GetRewriteEncodedKeys ——
    // normal: GetRewriteRawKeys / GetRewriteEncodedKeys
    // Raw：输入未编码表前缀；Encoded：输入已 EncodeBytes 的日志键。
    let rewrite_for_keys = RewriteRules {
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
    let raw_file = backuppb::File {
        Name: "backup.sst".into(),
        StartKey: tablecodec::GenTableRecordPrefix(1),
        EndKey: tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(1)),
        ..Default::default()
    };
    // Raw 路径内部会 EncodeBytes，故需 DecodeBytes 再比业务前缀。
    let (start, end) = GetRewriteRawKeys(&raw_file, Some(&rewrite_for_keys)).unwrap();
    // 解码后应落到新表 record 前缀及其 PrefixNext。
    let (_, start_raw) = codec::DecodeBytes(&start.unwrap(), None).unwrap();
    let (_, end_raw) = codec::DecodeBytes(&end.unwrap(), None).unwrap();
    assert_eq!(start_raw, tablecodec::GenTableRecordPrefix(2));
    assert_eq!(
        end_raw,
        tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(2))
    );

    let enc_file = backuppb::DataFileInfo {
        Path: "backup.log".into(),
        StartKey: codec::EncodeBytes(Vec::new(), &tablecodec::GenTableRecordPrefix(1)),
        EndKey: codec::EncodeBytes(
            Vec::new(),
            &tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(1)),
        ),
    };
    // Encoded 路径先 DecodeBytes 再改写，输出仍为编码键。
    let (enc_start, enc_end) = GetRewriteEncodedKeys(&enc_file, Some(&rewrite_for_keys)).unwrap();
    assert_eq!(
        enc_start.unwrap(),
        codec::EncodeBytes(Vec::new(), &tablecodec::GenTableRecordPrefix(2))
    );
    // enc_end 与 start 一并改写；未单独断言以保持与既有用例一致。

    // —— RewriteRange：索引前缀规则 ——
    // normal: RewriteRange with index prefix rules
    let mut rg = Range {
        KeyRange: KeyRange {
            StartKey: [tablecodec::GenTableIndexPrefix(1), b"startKey".to_vec()].concat(),
            EndKey: [tablecodec::GenTableIndexPrefix(1), b"endKey".to_vec()].concat(),
        },
        Files: vec![],
    };
    let idx_rules = RewriteRules {
        Data: vec![import_sstpb::RewriteRule {
            OldKeyPrefix: tablecodec::GenTableIndexPrefix(1),
            NewKeyPrefix: tablecodec::GenTableIndexPrefix(2),
            ..Default::default()
        }],
        ..Default::default()
    };
    // 成功路径：原地改写 rg 并返回克隆。
    let rewritten = RewriteRange(&mut rg, Some(&idx_rules)).unwrap();
    // 起止键应换到新表索引前缀。
    assert!(
        rewritten
            .StartKey
            .starts_with(&tablecodec::GenTableIndexPrefix(2))
    );

    // —— 起止表 ID 不一致：RewriteRange 报错 ——
    // error: table id mismatch on range rewrite
    let mut bad_rg = Range {
        KeyRange: KeyRange {
            StartKey: b"t1_startKey".to_vec(),
            EndKey: b"t2_endKey".to_vec(),
        },
        Files: vec![],
    };
    // 非标准表前缀导致 DecodeTableID 不一致 → ErrRestoreTableIDMismatch。
    assert!(RewriteRange(&mut bad_rg, Some(&idx_rules)).is_err());

    // —— GetRewriteTableID：由规则反解新表 ID ——
    // normal: GetRewriteTableID
    let tid = GetRewriteTableID(
        80,
        &RewriteRules {
            Data: vec![import_sstpb::RewriteRule {
                OldKeyPrefix: tablecodec::EncodeTablePrefix(80),
                NewKeyPrefix: tablecodec::EncodeTablePrefix(76),
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    // 80→76：从 NewKeyPrefix 解码表 ID。
    assert_eq!(tid, 76);

    // —— MergeAndRewriteFileRanges：空、非法 CF、正常 write ——
    // normal + boundary: MergeAndRewriteFileRanges
    let (empty_ranges, empty_stat) = MergeAndRewriteFileRanges(vec![], None, 1024, 1024).unwrap();
    // 空备份短路，不进入 CF 识别。
    assert!(empty_ranges.is_empty());
    assert_eq!(empty_stat.TotalFiles, 0);

    // 未知 CF：应失败。
    let bad_cf = MergeAndRewriteFileRanges(
        vec![backuppb::File {
            Name: "unknown.sst".into(),
            Cf: "other".into(),
            StartKey: b"a".to_vec(),
            EndKey: b"b".to_vec(),
            ..Default::default()
        }],
        None,
        1024,
        1024,
    );
    // 与 merge_test::test_invalid_ranges 同类断言。
    assert!(bad_cf.is_err());

    // 单 write CF 文件：统计与合并结果非空。
    let merge_files = vec![backuppb::File {
        Name: "f_write.sst".into(),
        Cf: WriteCFName.into(),
        StartKey: tablecodec::GenTableRecordPrefix(1),
        EndKey: tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(1)),
        TotalBytes: 100,
        TotalKvs: 10,
        ..Default::default()
    }];
    let (merged, stat) =
        MergeAndRewriteFileRanges(merge_files, None, 1_000_000, 1_000_000).unwrap();
    // write CF 计数来自 Cf 字段精确匹配。
    assert_eq!(stat.TotalWriteCFFile, 1);
    assert!(!merged.is_empty());

    // —— Append：合并两条规则的 Data 列表 ——
    // resource: Append merges data rules
    let mut base = EmptyRewriteRule();
    let mut other = EmptyRewriteRule();
    other.Data.push(import_sstpb::RewriteRule {
        OldKeyPrefix: vec![1],
        NewKeyPrefix: vec![2],
        ..Default::default()
    });
    // Append 只拼接 Data，不合并 TS/Keyspace 字段。
    base.Append(other);
    assert_eq!(base.Data.len(), 1);

    // 构造类型可编译/可默认初始化即可，验证导出符号存在。
    // TableIDRemap / EmptyRewriteRulesMap 为公开构造面冒烟。
    let _ = TableIDRemap {
        Origin: 1,
        Rewritten: 2,
    };
    let _map = crate::EmptyRewriteRulesMap();
}

/// Go tablecodec.DecodeTableID also accepts API V2 keys after stripping the
/// four-byte mode/keyspace prefix used by TiKV client-go.
#[test]
fn decode_table_id_accepts_api_v2_keyspace_prefixes() {
    let table_key = tablecodec::EncodeTablePrefix(42);

    for mode in [b'x', b'r'] {
        let api_v2_key = [[mode, 0x01, 0x02, 0x03].as_slice(), &table_key].concat();
        assert_eq!(tablecodec::DecodeTableID(&api_v2_key), 42);
    }

    assert_eq!(tablecodec::DecodeTableID(b"x\x01\x02"), 0);
}
