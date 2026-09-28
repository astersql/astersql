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

// `SliceIter` 单元测试。
//
// 对应 Go `TestSliceIter`：覆盖 nil/空/多条目及关闭后行为。

use mock_crate::{NewSliceIter, SliceIter, kv};

/// 构造测试用 KV Entry。
fn entry(key: &str, value: &str) -> kv::Entry {
    kv::Entry {
        Key: kv::Key(key.as_bytes().to_vec()),
        Value: value.as_bytes().to_vec(),
    }
}

/// 对应 Go `newSliceIterWithCopy`：`None` 保留 nil 用例，非空则先克隆再交由迭代器持有。
/// Mirrors Go's `newSliceIterWithCopy`: `None` retains the nil test case while
/// every non-nil slice is cloned before the iterator owns it.
fn new_slice_iter_with_copy(data: Option<&[kv::Entry]>) -> Box<SliceIter> {
    NewSliceIter(data.unwrap_or_default().to_vec())
}

/// 逐条比较 Entry 的 Key/Value。
fn assert_entries_equal(actual: &[kv::Entry], expected: &[kv::Entry]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.Key, expected.Key);
        assert_eq!(actual.Value, expected.Value);
    }
}

/// 直接对应 Go `TestSliceIter`：含正常迭代与关闭后校验。
/// Direct counterpart of Go `TestSliceIter`, including nil, empty, one-entry,
/// two-entry, and three-entry inputs plus both normal and post-close checks.
#[test]
fn test_slice_iter() {
    super::mock_main_test::setup_for_common_test();

    let cases = vec![
        None,
        Some(vec![]),
        Some(vec![entry("k1", "v1")]),
        Some(vec![entry("k1", "v1"), entry("k2", "v2")]),
        Some(vec![entry("k1", "v1"), entry("k0", ""), entry("k2", "v2")]),
    ];

    for data in cases {
        let original = data.as_deref().unwrap_or_default();

        // Normal iteration.
        let mut iter = new_slice_iter_with_copy(data.as_deref());
        for expected in original {
            assert!(iter.Valid());
            assert_eq!(iter.Key(), expected.Key);
            assert_eq!(iter.Value(), expected.Value);
            assert!(iter.Next().is_ok());
        }
        assert!(!iter.Valid());
        assert!(iter.Next().is_err());

        // Iteration must not modify the owned copy of the source slice.
        assert_entries_equal(iter.GetSlice(), original);

        // Iteration after close.
        let mut iter = new_slice_iter_with_copy(data.as_deref());
        assert_eq!(iter.Valid(), !original.is_empty());
        iter.Close();
        assert!(!iter.Valid());
        assert!(iter.Next().is_err());

        // Close must not modify the owned copy either.
        assert_entries_equal(iter.GetSlice(), original);
    }
}
