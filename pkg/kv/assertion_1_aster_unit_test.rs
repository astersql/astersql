// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// KV 包迁移对齐单元测试：断言位、键边界、HandleMap、分区范围与请求白名单等。
//
// 覆盖断言标志切换、Key/KeyRange 边界、CommonHandle 映射、
// `PartitionedKeyRanges` 排序与 hint 行为、请求类型检查器、
// `NextUntil` 迭代以及可重试/重复键错误生成，与 Go 行为对齐。

use super::*;
use std::collections::HashMap;

/// 断言操作与 KeyFlags 位切换应与 Go 一致，且不破坏其他既有标志位。
#[test]
fn assertion_and_key_flag_transitions_match_go() {
    // 先置入若干非断言标志，验证 ApplyAssertionOp 不会清除它们。
    let base = ApplyFlagsOps(
        KeyFlags::default(),
        &[
            FlagsOp::SetPresumeKeyNotExists,
            FlagsOp::SetNeedLocked,
            FlagsOp::SetNeedConstraintCheckInPrewrite,
            FlagsOp::SetPreviousPresumeKeyNotExists,
        ],
    );
    assert!(base.HasPresumeKeyNotExists());
    assert!(base.HasNeedLocked());
    assert!(base.HasNeedConstraintCheckInPrewrite());

    let exists = ApplyAssertionOp(base, AssertionOp::AssertExist);
    assert!(exists.HasAssertExists());
    assert!(!exists.HasAssertNotExists());
    assert!(exists.HasPresumeKeyNotExists());

    let not_exists = ApplyAssertionOp(exists, AssertionOp::AssertNotExist);
    assert!(!not_exists.HasAssertExists());
    assert!(not_exists.HasAssertNotExists());

    let unknown = ApplyAssertionOp(not_exists, AssertionOp::AssertUnknown);
    assert!(unknown.HasAssertUnknown());
    assert_eq!(ApplyAssertionOp(unknown, AssertionOp::AssertNone), unknown);
}

/// Key 后继、前缀后继与点范围判定边界应与 Go 一致。
#[test]
fn key_successors_and_point_ranges_match_go_boundaries() {
    assert_eq!(Key(vec![1, 2]).Next(), Key(vec![1, 2, 0]));
    assert_eq!(Key(vec![1, 2]).PrefixNext(), Key(vec![1, 3]));
    assert_eq!(Key(vec![1, 0xff, 0xff]).PrefixNext(), Key(vec![2, 0, 0]));
    assert_eq!(Key(vec![0xff]).PrefixNext(), Key(vec![0xff, 0]));
    assert_eq!(Key(Vec::new()).PrefixNext(), Key(vec![0]));

    // (start, end, is_point) 用例表，覆盖普通行键、空键与进位边界。
    let cases = [
        (b"rowkey1".to_vec(), b"rowkey2".to_vec(), true),
        (b"rowkey1".to_vec(), b"rowkey3".to_vec(), false),
        (Vec::new(), vec![0], true),
        (vec![123, 123, 255, 255], vec![123, 124, 0, 0], true),
        (vec![123, 123, 255, 255], vec![123, 124, 0, 1], false),
        (vec![123, 123], vec![123, 123, 0], true),
        (vec![255], vec![0], false),
    ];
    for (start, end, expected) in cases {
        assert_eq!(
            KeyRange {
                StartKey: Key(start),
                EndKey: Key(end)
            }
            .IsPoint(),
            expected
        );
    }
}

/// HandleMap 须把任意编码字节当作独立键，不能因 UTF-8 解释而合并。
#[test]
fn handle_maps_keep_arbitrary_encoded_bytes_distinct() {
    let mut first_encoded = vec![1];
    first_encoded = codec::EncodeBytes(first_encoded, &[0x80]);
    let mut second_encoded = vec![1];
    second_encoded = codec::EncodeBytes(second_encoded, &[0x81]);
    let first = NewCommonHandle(first_encoded).expect("first handle");
    let second = NewCommonHandle(second_encoded).expect("second handle");
    let mut map = NewHandleMap();
    map.Set(&first, Box::new("first".to_owned()));
    map.Set(&second, Box::new("second".to_owned()));

    assert_eq!(map.Len(), 2, "Go string keys preserve arbitrary bytes");
    assert_eq!(
        map.Get(&first).and_then(|v| v.downcast_ref::<String>()),
        Some(&"first".to_owned())
    );
    assert_eq!(
        map.Get(&second).and_then(|v| v.downcast_ref::<String>()),
        Some(&"second".to_owned())
    );
}

/// 分区键范围按 StartKey 排序时，旁路传入的 hints 保持原槽位与顺序（对齐 Go）。
#[test]
fn partitioned_key_ranges_sort_with_go_hint_behavior() {
    /// 构造单字节起止的简易范围。
    fn range(start: u8) -> KeyRange {
        KeyRange {
            StartKey: Key(vec![start]),
            EndKey: Key(vec![start + 1]),
        }
    }

    let mut ranges = NewPartitionedKeyRangesWithHints(
        vec![vec![range(4), range(3)], vec![range(2), range(1)]],
        vec![vec![40, 30], vec![20, 10]],
    );
    ranges.SortByFunc(|a, b| a.StartKey.0.cmp(&b.StartKey.0));

    let mut observed = Vec::new();
    ranges
        .ForEachPartitionWithErr(|partition, hints| {
            observed.push((
                partition
                    .iter()
                    .map(|r| r.StartKey.0[0])
                    .collect::<Vec<_>>(),
                hints.to_vec(),
            ));
            Ok(())
        })
        .expect("range callback");
    // Go sorts only `ranges`; the separately supplied hints remain in their
    // original slots and order. Preserve that exact behavior for compatibility.
    assert_eq!(
        observed,
        vec![(vec![1, 2], vec![40, 30]), (vec![3, 4], vec![20, 10])]
    );
    assert!(ranges.IsFullySorted());
}

/// 请求类型白名单：Select/DAG/Analyze 支持集合与 Go 一致，Checksum 不支持。
#[test]
fn request_type_checker_matches_go_whitelist() {
    let checker = RequestTypeSupportedChecker;
    assert!(checker.IsRequestTypeSupported(ReqTypeSelect, ReqSubTypeGroupBy));
    assert!(checker.IsRequestTypeSupported(ReqTypeDAG, ReqSubTypeSignature));
    assert!(checker.IsRequestTypeSupported(ReqTypeDAG, ReqSubTypeDesc));
    assert!(checker.IsRequestTypeSupported(ReqTypeSelect, ExprType::SumInt as i64));
    assert!(!checker.IsRequestTypeSupported(ReqTypeDAG, ReqSubTypeAnalyzeIdx));
    assert!(checker.IsRequestTypeSupported(ReqTypeAnalyze, 0));
    assert!(!checker.IsRequestTypeSupported(ReqTypeChecksum, 0));
}

/// 固定返回指定 value 的 Snapshot，用于验证 cacheDB 写回时的 freecache 边界。
struct CacheBoundarySnapshot {
    value: Vec<u8>,
}

impl Getter for CacheBoundarySnapshot {
    fn Get(
        &self,
        _ctx: &context::Context,
        _key: Key,
        _options: &[GetOption],
    ) -> Result<ValueEntry, errors::SharedError> {
        Ok(NewValueEntry(self.value.clone(), 0))
    }
}

impl Retriever for CacheBoundarySnapshot {
    fn Iter(
        &self,
        _key: Key,
        _upper_bound: Option<Key>,
    ) -> Result<Box<dyn Iterator>, errors::SharedError> {
        panic!("cacheDB UnionGet must not call Iter")
    }

    fn IterReverse(
        &self,
        _key: Option<Key>,
        _lower_bound: Option<Key>,
    ) -> Result<Box<dyn Iterator>, errors::SharedError> {
        panic!("cacheDB UnionGet must not call IterReverse")
    }
}

impl Snapshot for CacheBoundarySnapshot {
    fn BatchGet(
        &self,
        _ctx: &context::Context,
        _keys: &[Key],
        _options: &[BatchGetOption],
    ) -> Result<HashMap<String, ValueEntry>, errors::SharedError> {
        panic!("cacheDB UnionGet must not call BatchGet")
    }

    fn SetOption(&mut self, _opt: i32, _val: Option<Box<dyn std::any::Any>>) {
        panic!("cacheDB UnionGet must not call SetOption")
    }
}

/// cacheDB 写回必须保留 Go freecache 的大 key / 大 entry 拒绝边界与错误文本。
#[test]
fn cache_db_preserves_freecache_entry_limits() {
    let ctx = context::Context::todo();
    let cache = NewCacheDB();
    let small_snapshot = CacheBoundarySnapshot { value: vec![1] };
    let large_key_error = cache
        .UnionGet(&ctx, 1, &small_snapshot, &Key(vec![0; 65_536]))
        .expect_err("freecache rejects keys larger than uint16");
    assert_eq!(large_key_error.to_string(), "The key is larger than 65535");

    // 100 MiB cache / 1024 - 24-byte freecache entry header = 102376 bytes.
    let large_snapshot = CacheBoundarySnapshot {
        value: vec![1; 102_376],
    };
    let large_entry_error = cache
        .UnionGet(&ctx, 2, &large_snapshot, &Key(vec![0]))
        .expect_err("freecache rejects entries larger than one cache shard quarter");
    assert_eq!(
        large_entry_error.to_string(),
        "The entry size is larger than 1/1024 of cache size"
    );
}

/// 测试用迭代器：可预设键序列，并在指定位置让 Next 返回错误。
struct TestIterator {
    /// 有序键序列。
    keys: Vec<Key>,
    /// 当前游标位置。
    position: usize,
    /// 若为 Some(i)，则在 position==i 时 Next 失败。
    fail_at: Option<usize>,
}

impl Iterator for TestIterator {
    fn Valid(&self) -> bool {
        self.position < self.keys.len()
    }

    fn Key(&self) -> Key {
        self.keys[self.position].clone()
    }

    fn Value(&self) -> Vec<u8> {
        Vec::new()
    }

    fn Next(&mut self) -> Result<(), errors::SharedError> {
        if self.fail_at == Some(self.position) {
            return Err(errors::New("next failed"));
        }
        self.position += 1;
        Ok(())
    }

    fn Close(&mut self) {}
}

/// NextUntil 在命中谓词时停下，并向上传递 Next 返回的错误。
#[test]
fn next_until_stops_on_match_and_propagates_next_error() {
    /// 谓词：当前键等于 `[2]`。
    fn at_two(key: Key) -> bool {
        key == Key(vec![2])
    }
    let mut iter = TestIterator {
        keys: vec![Key(vec![1]), Key(vec![2]), Key(vec![3])],
        position: 0,
        fail_at: None,
    };
    NextUntil(&mut iter, at_two).expect("stop at matching key");
    assert_eq!(iter.position, 1);

    // Go checks Valid before Key, and the predicate before advancing.
    for (keys, expected_position) in [
        (vec![], 0),
        (vec![Key(vec![2])], 0),
        (vec![Key(vec![1]), Key(vec![3])], 2),
    ] {
        let mut boundary = TestIterator {
            keys,
            position: 0,
            fail_at: Some(expected_position),
        };
        NextUntil(&mut boundary, at_two).expect("empty, matched, or exhausted iterator");
        assert_eq!(boundary.position, expected_position);
    }

    let mut failing = TestIterator {
        keys: vec![Key(vec![1]), Key(vec![2])],
        position: 0,
        fail_at: Some(0),
    };
    assert_eq!(
        NextUntil(&mut failing, at_two)
            .expect_err("next error")
            .to_string(),
        "next failed"
    );
    assert_eq!(failing.position, 0, "failed Next must not advance");
}

/// 可重试事务错误判定与重复键错误文案应对齐 Go。
#[test]
fn retryable_and_duplicate_key_errors_match_go() {
    let retryable = ErrTxnRetryable.FastGenByArgs(&[]);
    let conflict = ErrWriteConflict.FastGenByArgs(&[]);
    assert!(!IsTxnRetryableError(None));
    assert!(IsTxnRetryableError(Some(&retryable)));
    assert!(IsTxnRetryableError(Some(&conflict)));
    assert!(IsTxnRetryableError(Some(
        &ErrWriteConflictInTiDB.FastGenByArgs(&[])
    )));
    assert!(!IsTxnRetryableError(Some(&errors::New("plain"))));
    assert_eq!(
        GenKeyExistsErr(&["one".into(), "two".into()], "PRIMARY").to_string(),
        "[kv:1062]Duplicate entry 'one-two' for key 'PRIMARY'"
    );
}
