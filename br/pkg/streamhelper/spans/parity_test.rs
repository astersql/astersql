// Copyright 2026 AsterSQL.

//! spans 公开契约的 Go/Rust 对齐冒烟测试。
//!
//! 覆盖 Overlaps/Collapse、ValuedFull.Merge/Traverse、SortedFull.MinValue 与 Sorted 包装，
//! 不改行为，仅锁定与 Go `spans` 包一致的关键语义。
//!
//! 失败通常意味着区间几何或 Merge 取值策略漂移，需对照 `utils.go`/`sorted.go`。

use crate::{Collapse, Full, NewFullWith, NewSortedFull, Overlaps, Sorted, Span, Valued};

#[test]
fn go_rust_public_contract_matches() {
    // 交叉区间应重叠；不相交半开区间不应重叠。
    let a = Span {
        StartKey: b"a".to_vec(),
        EndKey: b"c".to_vec(),
    };
    let b = Span {
        StartKey: b"b".to_vec(),
        EndKey: b"d".to_vec(),
    };
    assert!(Overlaps(&a, &b));
    // `[a,b)` 与 `[c,d)` 无交集。
    // 半开语义：端点相接不算重叠。
    assert!(!Overlaps(
        &Span {
            StartKey: b"a".to_vec(),
            EndKey: b"b".to_vec()
        },
        &Span {
            StartKey: b"c".to_vec(),
            EndKey: b"d".to_vec()
        }
    ));

    // 重叠输入折叠为单一覆盖跨度 `[1,9)`。
    // 三段递进重叠最终并成一条。
    let collapsed = Collapse(&[
        Span {
            StartKey: b"1".to_vec(),
            EndKey: b"4".to_vec(),
        },
        Span {
            StartKey: b"2".to_vec(),
            EndKey: b"8".to_vec(),
        },
        Span {
            StartKey: b"3".to_vec(),
            EndKey: b"9".to_vec(),
        },
    ]);
    assert_eq!(collapsed.len(), 1);
    assert_eq!(collapsed[0].StartKey, b"1");
    assert_eq!(collapsed[0].EndKey, b"9");

    // 全键空间初始化后 Merge 左半段，Traverse 至少产出一段。
    // 初始值 41，左半 Merge 到 50，右侧保留原值。
    let mut full = NewFullWith(&Full(), 41);
    full.Merge(Valued {
        Key: Span {
            StartKey: b"".to_vec(),
            EndKey: b"m".to_vec(),
        },
        Value: 50,
    });
    let mut count = 0;
    full.Traverse(|_| {
        count += 1;
        true
    });
    assert!(count >= 1);

    // SortedFull：Merge 更高值后 MinValue 不低于初始值。
    // 全局最小水位用于 advancer 安全推进。
    let mut vs = NewSortedFull(10);
    vs.Merge(Valued {
        Key: Span {
            StartKey: vec![],
            EndKey: b"z".to_vec(),
        },
        Value: 20,
    });
    assert!(vs.MinValue().unwrap() >= 10);

    // Sorted 包装仅做类型/构造可达性校验。
    // 确保再导出符号在测试配置下可链接。
    let _ = Sorted(NewFullWith(
        &[Span {
            StartKey: b"a".to_vec(),
            EndKey: b"b".to_vec(),
        }],
        1,
    ));
}

#[test]
fn remaining_go_contract_edges_match() {
    let span = |start: &[u8], end: &[u8]| Span {
        StartKey: start.to_vec(),
        EndKey: end.to_vec(),
    };
    let valued = |start: &[u8], end: &[u8], value| Valued {
        Key: span(start, end),
        Value: value,
    };

    // Go Collapse merges adjacent ranges, preserves gaps, and accepts empty input.
    assert!(Collapse(&[]).is_empty());
    assert_eq!(
        Collapse(&[span(b"d", b"f"), span(b"a", b"b"), span(b"b", b"d")]),
        vec![span(b"a", b"f")]
    );
    assert_eq!(
        Collapse(&[span(b"a", b"b"), span(b"c", b"d")]),
        vec![span(b"a", b"b"), span(b"c", b"d")]
    );

    // Empty EndKey is +infinity, while touching finite half-open ranges do not overlap.
    assert!(Overlaps(&span(b"a", b""), &span(b"z", b"")));
    assert!(!Overlaps(&span(b"a", b"b"), &span(b"b", b"c")));

    // Go Valued.Less/Equals compare start keys and exact valued ranges respectively.
    let first = valued(b"a", b"b", 1);
    let second = valued(b"b", b"c", 1);
    assert!(first.Less(&second));
    assert!(first.Equals(&first));
    assert!(!first.Equals(&valued(b"a", b"b", 2)));

    // Traverse must honor early termination rather than always scanning the tree.
    let full = NewFullWith(&[span(b"a", b"b"), span(b"c", b"d")], 7);
    let mut visited = 0;
    full.Traverse(|_| {
        visited += 1;
        false
    });
    assert_eq!(visited, 1);

    // Value ordering is strict (< n), stable by StartKey, and also honors early termination.
    let mut sorted = Sorted(NewFullWith(&[span(b"a", b"b"), span(b"c", b"d")], 3));
    sorted.MergeAll(vec![valued(b"a", b"b", 5), valued(b"c", b"d", 4)]);
    assert_eq!(sorted.MinValue(), Some(4));
    assert_eq!(sorted.Min(), Some(valued(b"c", b"d", 4)));
    let mut below_five = Vec::new();
    sorted.TraverseValuesLessThan(5, |item| {
        below_five.push(item);
        false
    });
    assert_eq!(below_five, vec![valued(b"c", b"d", 4)]);

    // Rust exposes the Go non-empty invariant as Option so an empty wrapped tree is safe.
    let empty = Sorted(NewFullWith(&[], 0));
    assert_eq!(empty.Min(), None);
    assert_eq!(empty.MinValue(), None);
}
