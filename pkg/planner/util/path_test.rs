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

// AccessPath 相关工具函数的单元测试。
//
// 覆盖列长度映射比较（`CompareCol2Len`）以及访问路径是否仅含点查范围
// （`OnlyPointRange`）。AccessPath 是优化器为表/索引选择的一种访问路径；
// 点查范围（point range）指 LowVal 与 HighVal 相等的单点谓词。

use std::collections::HashMap;

use super::{
    AccessPath, Col2Len, CompareCol2Len, IndexLookUpPushDownByHint, IndexLookUpPushDownNone,
};
use expression::types;

/// 验证 `CompareCol2Len`：比较两份列 ID→前缀长度映射的优劣与可比性。
/// Col2Len 中 `-1` 表示整列（无前缀限制）；不可比时仍返回偏序结果。
#[test]
fn test_compare_col2_len() {
    crate::main_test::setup_for_planner_util_test();
    /// 单组左右映射、期望比较结果与是否可比。
    struct Case {
        left: Col2Len,
        right: Col2Len,
        result: i32,
        comparable: bool,
    }
    // 由 (列 ID, 前缀长度) 列表构造 HashMap。
    let map = |entries: &[(i64, isize)]| HashMap::from_iter(entries.iter().copied());
    let cases = vec![
        // left 列更多且均为整列，优于 right。
        Case {
            left: map(&[(1, -1), (2, -1), (3, -1)]),
            right: map(&[(1, -1), (2, 10)]),
            result: 1,
            comparable: true,
        },
        // left 前缀更短，劣于 right。
        Case {
            left: map(&[(1, 5)]),
            right: map(&[(1, 10), (2, -1)]),
            result: -1,
            comparable: true,
        },
        // 列集合不一致，不可比。
        Case {
            left: map(&[(1, -1), (2, -1)]),
            right: map(&[(1, -1), (2, 5), (3, -1)]),
            result: -1,
            comparable: false,
        },
        Case {
            left: map(&[(1, -1), (2, 10)]),
            right: map(&[(1, -1), (2, 5), (3, -1)]),
            result: -1,
            comparable: false,
        },
        // 完全相同，相等且可比。
        Case {
            left: map(&[(1, -1), (2, 10)]),
            right: map(&[(1, -1), (2, 10)]),
            result: 0,
            comparable: true,
        },
        // 列集合相同但长度语义不同导致不可比。
        Case {
            left: map(&[(1, -1), (2, -1)]),
            right: map(&[(1, -1), (2, 10)]),
            result: -1,
            comparable: false,
        },
    ];

    for case in cases {
        assert_eq!(
            CompareCol2Len(&case.left, &case.right),
            (case.result, case.comparable)
        );
    }
}

/// Go's `AccessPath.Clone` only copies the fields named in its struct literal.
/// Fields omitted there retain their zero values in the clone.
#[test]
fn test_access_path_clone_preserves_go_zero_value_semantics() {
    crate::main_test::setup_for_planner_util_test();
    let original = AccessPath {
        IndexMergeAccessMVIndex: true,
        IndexLookUpPushDownBy: IndexLookUpPushDownByHint,
        ..Default::default()
    };

    let cloned = original.Clone();

    assert!(!cloned.IndexMergeAccessMVIndex);
    assert_eq!(cloned.IndexLookUpPushDownBy, IndexLookUpPushDownNone);
}

/// 构造单列点查 Range：LowVal/HighVal 均为同一 Datum。
fn point(value: expression::types::Datum) -> ranger::Range {
    ranger::Range {
        LowVal: vec![value.clone()],
        HighVal: vec![value],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Default::default()
    }
}

/// 验证 `OnlyPointRange`：整型句柄与单列索引在仅点查时可返回 true，
/// 区间范围、NULL 点（非句柄）、多列索引则返回 false。
#[test]
fn test_only_point_range() {
    crate::main_test::setup_for_planner_util_test();
    // NULL Datum 也可用作点查边界（整型句柄路径允许）。
    let mut null = types::MinNotNullDatum();
    null.SetNull();
    let null_point = point(null);
    let one_point = point(types::NewIntDatum(1));
    // 区间 [1, 2]，非点查。
    let one_to_two = ranger::Range {
        LowVal: vec![types::NewIntDatum(1)],
        HighVal: vec![types::NewIntDatum(2)],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Default::default()
    };
    let type_context = (*types::DefaultStmtNoWarningContext).clone();

    // 整型句柄路径：仅点查为 true，混入区间后为 false。
    let mut int_handle = AccessPath {
        IsIntHandlePath: true,
        Ranges: vec![null_point.clone(), one_point.clone()],
        ..Default::default()
    };
    assert!(int_handle.OnlyPointRange(type_context.clone()));
    int_handle.Ranges = vec![one_point.clone(), one_to_two.clone()];
    assert!(!int_handle.OnlyPointRange(type_context.clone()));

    // 单列索引：单点为 true；含 NULL 或区间为 false。
    let mut index = AccessPath {
        Index: Some(model::IndexInfo {
            Columns: vec![model::IndexColumn::default()],
            ..Default::default()
        }),
        Ranges: vec![one_point.clone()],
        ..Default::default()
    };
    assert!(index.OnlyPointRange(type_context.clone()));
    index.Ranges = vec![null_point, one_point.clone()];
    assert!(!index.OnlyPointRange(type_context.clone()));
    index.Ranges = vec![one_point.clone(), one_to_two];
    assert!(!index.OnlyPointRange(type_context.clone()));

    // 多列索引即使只有一个点查 Range 也不视为 OnlyPointRange。
    index.Index.as_mut().unwrap().Columns =
        vec![model::IndexColumn::default(), model::IndexColumn::default()];
    index.Ranges = vec![one_point];
    assert!(!index.OnlyPointRange(type_context));
}
