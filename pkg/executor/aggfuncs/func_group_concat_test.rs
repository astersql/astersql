// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// GROUP_CONCAT 聚合的单元测试。
//
// 可执行用例覆盖 DISTINCT 去重、NULL 跳过、分隔符拼接与最大长度截断。

use crate::func_group_concat::GroupConcat;

/// 校验 DISTINCT+分隔符拼接，以及非 DISTINCT 下超长截断与 truncated 标志。
#[test]
fn group_concat_applies_separator_distinct_and_maximum_length() {
    // DISTINCT：重复 "a" 只保留一次；NULL 被跳过；结果为 "a,b"。
    let mut concat = GroupConcat::new(b",".to_vec(), 64, true);
    concat.update([
        Some(b"a".to_vec()),
        None,
        Some(b"a".to_vec()),
        Some(b"b".to_vec()),
    ]);
    assert_eq!(concat.result(), Some(b"a,b".as_slice()));
    assert!(!concat.truncated());

    // maximum_len=4："ab|cd" 截成 "ab|c" 并标记 truncated。
    let mut truncated = GroupConcat::new(b"|".to_vec(), 4, false);
    truncated.update([Some(b"ab".to_vec()), Some(b"cd".to_vec())]);
    assert_eq!(truncated.result(), Some(b"ab|c".as_slice()));
    assert!(truncated.truncated());
}

/// Go `groupConcat` keeps a non-NULL empty string distinct from an empty group.
/// It also treats max_len=0 as unlimited and preserves separators when a
/// partial result containing an empty value is merged.
#[test]
fn group_concat_preserves_empty_non_null_values_and_zero_limit() {
    let mut empty = GroupConcat::new(b"|".to_vec(), 0, false);
    empty.update([Some(Vec::new())]);
    assert_eq!(empty.result(), Some(b"".as_slice()));

    let mut unlimited = GroupConcat::new(b"|".to_vec(), 0, false);
    unlimited.update([Some(b"a".to_vec()), Some(b"b".to_vec())]);
    assert_eq!(unlimited.result(), Some(b"a|b".as_slice()));
    assert!(!unlimited.truncated());

    let mut merged = GroupConcat::new(b",".to_vec(), 64, false);
    merged.update([Some(b"a".to_vec())]);
    let mut empty_partial = GroupConcat::new(b",".to_vec(), 64, false);
    empty_partial.update([Some(Vec::new())]);
    merged.merge(&empty_partial);
    assert_eq!(merged.result(), Some(b"a,".as_slice()));

    let mut distinct = GroupConcat::new(b",".to_vec(), 64, true);
    distinct.update([Some(Vec::new()), Some(Vec::new()), Some(b"x".to_vec())]);
    assert_eq!(distinct.result(), Some(b",x".as_slice()));
}

/// Go keeps the truncation sentinel on the aggregate function rather than in
/// each partial result, so resetting between groups must not allow a second
/// truncation warning during the same aggregate function's lifetime.
#[test]
fn group_concat_reset_preserves_lifetime_truncation_sentinel() {
    let mut concat = GroupConcat::new(b",".to_vec(), 2, false);
    concat.update([Some(b"abc".to_vec())]);
    assert!(concat.truncated());

    concat.reset();
    assert_eq!(concat.result(), None);
    assert!(concat.truncated());
}
