// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/rtree` vs Go sources.
//!
//! 单测聚合校验 Rust `br/pkg/rtree` 与 Go 公开合约：KeyRange 半开语义、
//! RangeTree 缺口、NeedsMerge 同表约束、MergedRanges 聚合、ProgressRangeTree
//! 回调与重叠拒绝，以及 ZapRanges/SummaryFiles 辅助输出。
//! 无网络/TiKV；替身 MetaWriter 只计数 Send 次数。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    AppendDataFile, EncodeIndexKeyPrefix, EncodeRowKeyPrefix, File, KeyRange, MetaWriter,
    NeedsMerge, NewProgressRangeTree, NewRangeStatsTree, NewRangeTree, ProgressRange, Range,
    RangeStats, SummaryFiles, ZapRanges,
};

/// 内存 MetaWriter：累计发送的 File 条数，供进度树副作用断言。
struct MemWriter {
    // 原子计数：并发安全，虽本测为单线程写入。
    sent: AtomicUsize,
}
impl MetaWriter for MemWriter {
    fn Send(&self, files: &[File], _kind: i32) -> Result<(), String> {
        // 不关心 kind；包装层 W 会额外校验 AppendDataFile。
        // 按条数累加，而非按调用次数。
        self.sent.fetch_add(files.len(), Ordering::SeqCst);
        Ok(())
    }
}

/// 端到端公开面回归：顺序覆盖区间、树、合并、进度与摘要辅助。
#[test]
fn go_rust_public_contract_matches() {
    // normal: KeyRange Contains / Intersect
    // 半开 [a,c)：含 a/b，不含 c；ContainsRange 要求整段落入。
    let rg = KeyRange {
        StartKey: b"a".to_vec(),
        EndKey: b"c".to_vec(),
    };
    // 下界闭：起点 a 属于区间。
    assert!(rg.Contains(b"a"));
    // 内部点同样属于。
    assert!(rg.Contains(b"b"));
    // 上界开：终点 c 不属于。
    assert!(!rg.Contains(b"c"));
    // 子区间 [a,b) 完全落在 [a,c) 内。
    assert!(rg.ContainsRange(b"a", b"b"));
    // 右端超出 EndKey 则整段不包含。
    assert!(!rg.ContainsRange(b"a", b"d"));
    // 与 [b,d) 求交：左取 max，右取 min。
    let (s, e, ok) = rg.Intersect(b"b", b"d");
    assert!(ok);
    // 相交结果右端被裁到 c。
    assert_eq!(s, b"b");
    assert_eq!(e, b"c");

    // EndKey 空表示上界开放，任意更大键仍 Contains。
    let open = KeyRange {
        StartKey: b"a".to_vec(),
        EndKey: Vec::new(),
    };
    assert!(open.Contains(b"zzzz"));

    // RangeTree Put / Find / GetIncompleteRange
    // 放入 [a,b)+[c,d)，查询 [a,d) 应得到缺口 [b,c)。
    let mut tree = NewRangeTree();
    tree.Put(
        b"a".to_vec(),
        b"b".to_vec(),
        vec![File {
            TotalBytes: 10,
            TotalKvs: 1,
            Crc64Xor: 1,
            ..Default::default()
        }],
    );
    // 第二段无文件，仍占位覆盖区间。
    tree.Put(b"c".to_vec(), b"d".to_vec(), vec![]);
    let found = tree
        .Find(&Range {
            KeyRange: KeyRange {
                StartKey: b"a".to_vec(),
                EndKey: Vec::new(),
            },
            Files: vec![],
        })
        .unwrap();
    // Find 命中首段起点 a。
    assert_eq!(found.KeyRange.StartKey, b"a");
    // [a,d) 内仅 [b,c) 未覆盖。
    let gaps = tree.GetIncompleteRange(b"a".to_vec(), b"d".to_vec());
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].StartKey, b"b");
    assert_eq!(gaps[0].EndKey, b"c");

    // NeedsMerge: same table record keys merge
    // 同行表前缀且未超阈值 → 合并；跨表 ID → 不合并。
    let left = RangeStats {
        Range: Range {
            KeyRange: KeyRange {
                StartKey: EncodeRowKeyPrefix(1),
                EndKey: b"x".to_vec(),
            },
            Files: vec![File {
                TotalBytes: 10,
                TotalKvs: 1,
                Crc64Xor: 1,
                ..Default::default()
            }],
        },
        Size: 10,
        Count: 1,
    };
    let right_same = RangeStats {
        Range: Range {
            KeyRange: KeyRange {
                StartKey: EncodeRowKeyPrefix(1),
                EndKey: b"y".to_vec(),
            },
            Files: vec![File {
                TotalBytes: 10,
                TotalKvs: 1,
                Crc64Xor: 2,
                ..Default::default()
            }],
        },
        Size: 10,
        Count: 1,
    };
    let right_diff = RangeStats {
        Range: Range {
            KeyRange: KeyRange {
                // 表 ID 2：与 left 不同物理表，禁止合并。
                StartKey: EncodeRowKeyPrefix(2),
                EndKey: b"y".to_vec(),
            },
            Files: vec![File {
                TotalBytes: 10,
                TotalKvs: 1,
                Crc64Xor: 3,
                ..Default::default()
            }],
        },
        Size: 10,
        Count: 1,
    };
    // 同表且远低于 size/count 阈值 → true。
    assert!(NeedsMerge(&left, &right_same, 1000, 1000));
    // 表 ID 不同 → false，即使阈值充裕。
    assert!(!NeedsMerge(&left, &right_diff, 1000, 1000));
    // 同表不同 index id：索引键不允许跨索引合并。
    let idx1 = RangeStats {
        Range: Range {
            KeyRange: KeyRange {
                StartKey: EncodeIndexKeyPrefix(1, 7),
                EndKey: b"x".to_vec(),
            },
            Files: vec![File {
                TotalBytes: 5,
                TotalKvs: 1,
                Crc64Xor: 1,
                ..Default::default()
            }],
        },
        ..Default::default()
    };
    let idx2 = RangeStats {
        Range: Range {
            KeyRange: KeyRange {
                StartKey: EncodeIndexKeyPrefix(1, 8),
                EndKey: b"y".to_vec(),
            },
            Files: vec![File {
                TotalBytes: 5,
                TotalKvs: 1,
                Crc64Xor: 1,
                ..Default::default()
            }],
        },
        ..Default::default()
    };
    // index 7 vs 8：不同索引不得合并。
    assert!(!NeedsMerge(&idx1, &idx2, 1000, 1000));

    // MergedRanges
    // 同表两段相邻统计在高阈值下应合并为 Size=20 的一段。
    let mut st = NewRangeStatsTree();
    let mut k2 = EncodeRowKeyPrefix(1);
    // 追加 0xff 使第二段 StartKey 字典序紧随第一段前缀之后。
    k2.extend_from_slice(b"\xff");
    st.InsertRange(
        Range {
            KeyRange: KeyRange {
                StartKey: EncodeRowKeyPrefix(1),
                EndKey: b"b".to_vec(),
            },
            Files: vec![File {
                TotalBytes: 10,
                TotalKvs: 1,
                Crc64Xor: 1,
                ..Default::default()
            }],
        },
        10,
        1,
    );
    st.InsertRange(
        Range {
            KeyRange: KeyRange {
                StartKey: k2,
                EndKey: b"c".to_vec(),
            },
            Files: vec![File {
                TotalBytes: 10,
                TotalKvs: 1,
                Crc64Xor: 2,
                ..Default::default()
            }],
        },
        10,
        1,
    );
    // 高阈值下两段合并为一条。
    let merged = st.MergedRanges(1000, 1000);
    assert_eq!(merged.len(), 1);
    // Size 为两侧 Size 之和。
    assert_eq!(merged[0].Size, 20);

    // ProgressRangeTree
    // 首次 Insert 触发回调与 MetaWriter.Send；重叠 Origin 必须报错。
    let writer = Arc::new(MemWriter {
        sent: AtomicUsize::new(0),
    });
    // 包装层断言 kind==AppendDataFile，再委托计数。
    struct W(Arc<MemWriter>);
    impl MetaWriter for W {
        fn Send(&self, files: &[File], kind: i32) -> Result<(), String> {
            // 进度树完成时应发送 AppendDataFile 类型。
            assert_eq!(kind, AppendDataFile);
            // 转发给共享 MemWriter 累计条数。
            self.0.Send(files, kind)
        }
    }
    // false：非 checksum-only 模式，Insert 会写 meta。
    let mut prt = NewProgressRangeTree(Some(Box::new(W(Arc::clone(&writer)))), false);
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::clone(&calls);
    // 进度完成回调：用于外部推进 UI/计量。
    prt.SetCallBack(Box::new(move || {
        c2.fetch_add(1, Ordering::SeqCst);
    }));
    let mut res = NewRangeTree();
    // PhysicalID=42：写入校验和映射的键。
    res.PhysicalID = 42;
    res.Put(
        b"a".to_vec(),
        b"z".to_vec(),
        vec![File {
            TotalBytes: 3,
            TotalKvs: 1,
            Crc64Xor: 9,
            ..Default::default()
        }],
    );
    // Origin=[a,z) 且 Res 已完整覆盖 → 立即完成并触发回调。
    prt.Insert(ProgressRange {
        Res: res,
        Origin: KeyRange {
            StartKey: b"a".to_vec(),
            EndKey: b"z".to_vec(),
        },
    })
    .unwrap();
    // 与已插入 Origin 重叠 → overlapping 错误（英文片段兼容）。
    let err = prt
        .Insert(ProgressRange {
            Res: NewRangeTree(),
            Origin: KeyRange {
                StartKey: b"b".to_vec(),
                EndKey: b"c".to_vec(),
            },
        })
        .unwrap_err();
    // 错误文案兼容 overlapping/overlap 两种拼写。
    assert!(err.contains("overlapping") || err.contains("overlap"));

    // 完整覆盖后无缺口；回调与 Send 各恰好一次。
    let incomplete = prt.GetIncompleteRanges().unwrap();
    assert!(incomplete.is_empty());
    // 回调计数：完成一次进度。
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // MetaWriter 收到恰好一份文件列表。
    assert_eq!(writer.sent.load(Ordering::SeqCst), 1);
    // 校验和映射按 PhysicalID 索引。
    assert!(prt.GetChecksumMap().contains_key(&42));

    // ZapRanges 输出 JSON 对象壳；SummaryFiles 第二分量累加 TotalKvs。
    let s = ZapRanges(&[rg]).encode_json();
    // 必须含 ranges 字段名，形状与 logging 单测一致。
    assert!(s.contains("\"ranges\":"));
    // 整体是单个 JSON object。
    assert!(s.starts_with('{') && s.ends_with('}'));
    // SummaryFiles 返回 (bytes, kvs)；此处只断言 kvs=2。
    assert_eq!(
        SummaryFiles(&[File {
            TotalBytes: 1,
            TotalKvs: 2,
            Crc64Xor: 3,
            ..Default::default()
        }])
        .1,
        2
    );
}
