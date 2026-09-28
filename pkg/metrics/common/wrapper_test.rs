// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `GetMergedConstLabels` 单元测试：空集、并集与包级标签优先覆盖。

use super::{CONST_LABELS_TEST_LOCK, GetConstLabels, GetMergedConstLabels, Labels, SetConstLabels};

/// 由键值对切片构造 [`Labels`]。
fn labels(entries: &[(&str, &str)]) -> Labels {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// 测试结束时恢复进入前的包级常量标签。
struct ConstLabelsGuard(Labels);

impl Drop for ConstLabelsGuard {
    fn drop(&mut self) {
        let kv = self
            .0
            .iter()
            .flat_map(|(key, value)| [key.clone(), value.clone()])
            .collect::<Vec<_>>();
        SetConstLabels(&kv);
    }
}

// test_get_merged_const_labels corresponds to Go's TestGetMergedConstLabels.
/// 对照 Go `TestGetMergedConstLabels`：空输入、合并增补键、以及全局覆盖同名输入键。
#[test]
fn test_get_merged_const_labels() {
    let _serial = CONST_LABELS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _guard = ConstLabelsGuard(GetConstLabels());

    SetConstLabels(&[]);
    assert_eq!(Labels::new(), GetMergedConstLabels(Labels::new()));
    assert_eq!(
        labels(&[("c", "3")]),
        GetMergedConstLabels(labels(&[("c", "3")]))
    );

    SetConstLabels(&["a".into(), "1".into(), "b".into(), "2".into()]);
    assert_eq!(
        labels(&[("a", "1"), ("b", "2")]),
        GetMergedConstLabels(Labels::new())
    );
    assert_eq!(
        labels(&[("a", "1"), ("b", "2")]),
        GetMergedConstLabels(labels(&[]))
    );
    assert_eq!(
        labels(&[("a", "1"), ("b", "2"), ("c", "3")]),
        GetMergedConstLabels(labels(&[("c", "3")]))
    );

    // constLabels has higher priority
    // 包级 constLabels 优先级更高：输入同名键被覆盖。
    assert_eq!(
        labels(&[("a", "1"), ("b", "2")]),
        GetMergedConstLabels(labels(&[("a", "100")]))
    );
}
