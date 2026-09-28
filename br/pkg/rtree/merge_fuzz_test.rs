// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent of `br/pkg/rtree/merge_fuzz_test.go` `FuzzMerge`.
//! Pure algorithm — no network / TiKV. Go fuzz only requires NeedsMerge not panic.
//!
//! Rust 用固定语料模拟 Go fuzz 输入：种子索引键 + 空键/跨表/伪 keyspace 等边界。
//! 合约与 Go 相同——只要求 `NeedsMerge` 路径不 panic，不校验返回布尔值。

use crate::{EncodeIndexSeekKey, File, KeyRange, NeedsMerge, Range, RangeStats};

/// `FuzzMerge` → `fuzz_merge`.
///
/// Go seeds with `EncodeIndexSeekKey(42, 1, nil)` then fuzzes arbitrary a/b.
/// Rust exercises the seed plus a fixed corpus of edge-case keys (same contract:
/// call Must not panic).
///
/// 每对 (a,b) 构造左右 `RangeStats`（单文件、阈值 42/42），调用后丢弃返回值。
#[test]
fn fuzz_merge() {
    // Go fuzz 种子：table=42, index=1, 空 encoded values。
    let base = EncodeIndexSeekKey(42, 1, &[]);
    let corpus: Vec<(Vec<u8>, Vec<u8>)> = vec![
        // 同键自合并路径。
        (base.clone(), base.clone()),
        // 空键：DecodeKeyHead 失败分支仍不得 panic。
        (Vec::new(), Vec::new()),
        (b"a".to_vec(), b"b".to_vec()),
        (base.clone(), b"\xff".to_vec()),
        (vec![0], vec![255]),
        // 同表不同 index id：通常不应合并。
        (
            EncodeIndexSeekKey(1, 1, b"x"),
            EncodeIndexSeekKey(1, 2, b"y"),
        ),
        (
            EncodeIndexSeekKey(7, 3, &[]),
            EncodeIndexSeekKey(7, 3, b"z"),
        ),
        // keyspace-looking prefix that may fail DecodeKeyHead after strip
        // 伪 keyspace 前缀：剥离后可能解码失败，覆盖容错路径。
        (vec![b'x', 0, 0, 1, b't'], vec![b'x', 0, 0, 1, b'u']),
        (vec![1, 2, 3, 4, 5], vec![9, 8, 7]),
        // 以 't' 开头的定长缓冲，模拟表前缀形态但不保证合法编码。
        (
            {
                let mut v = vec![0u8; 20];
                v[0] = b't';
                v
            },
            {
                let mut v = vec![0xffu8; 20];
                v[0] = b't';
                v
            },
        ),
    ];

    // 阈值刻意偏小，迫使更多「是否合并」分支被走到，但仍不校验结果。
    for (a, b) in corpus {
        // EndKey 置空：半开上界开放，聚焦 StartKey 解码与合并判定。
        // Files 仅填最小统计，满足 NeedsMerge 读 Size/Count 前的结构约束。
        let left = RangeStats {
            Range: Range {
                KeyRange: KeyRange {
                    StartKey: a,
                    EndKey: Vec::new(),
                },
                Files: vec![File {
                    TotalKvs: 1,
                    TotalBytes: 1,
                    ..Default::default()
                }],
            },
            ..Default::default()
        };
        let right = RangeStats {
            Range: Range {
                KeyRange: KeyRange {
                    StartKey: b,
                    EndKey: Vec::new(),
                },
                Files: vec![File {
                    TotalKvs: 1,
                    TotalBytes: 1,
                    ..Default::default()
                }],
            },
            ..Default::default()
        };
        // Go does not check return value — only that the path does not panic.
        // 阈值 42/42 与 Go fuzz 默认一致；布尔结果故意忽略。
        let _ = NeedsMerge(&left, &right, 42, 42);
    }
}
