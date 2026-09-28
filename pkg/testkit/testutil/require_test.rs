// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `CompareUnorderedStringSlice` 的单元测试：重排、长度、重复计数与 nil/空切片语义。

use super::CompareUnorderedStringSlice;

/// 覆盖无序字符串多重集比较的主要分支。
#[test]
fn test_compare_unordered_string() {
    let original = ["1".to_owned(), "1".to_owned(), "2".to_owned()];
    let same = ["1".to_owned(), "1".to_owned(), "2".to_owned()];
    let reordered = ["1".to_owned(), "2".to_owned(), "1".to_owned()];
    let shorter = ["1".to_owned(), "1".to_owned()];
    let different_duplicates = ["1".to_owned(), "2".to_owned(), "2".to_owned()];
    let empty: [String; 0] = [];

    assert!(CompareUnorderedStringSlice(Some(&original), Some(&same)));
    assert!(CompareUnorderedStringSlice(
        Some(&original),
        Some(&reordered)
    ));
    assert!(!CompareUnorderedStringSlice(
        Some(&shorter),
        Some(&reordered)
    ));
    assert!(!CompareUnorderedStringSlice(
        Some(&original),
        Some(&different_duplicates)
    ));
    assert!(CompareUnorderedStringSlice(None, None));
    assert!(!CompareUnorderedStringSlice(Some(&empty), None));
    assert!(!CompareUnorderedStringSlice(None, Some(&empty)));
}
