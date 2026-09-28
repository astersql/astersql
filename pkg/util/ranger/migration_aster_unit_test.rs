// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// ranger 迁移补充单元测试。
//
// 覆盖 Range 字符串/单点语义、全范围与 Clone、区间求交、单列条件拆分，
// 以及 EQ/DNF 索引 Range 构造，对齐 Go 侧核心行为。

use crate::test_support_aster_unit_test::{equality, field_type, int_range, ranger_context};
use crate::{
    DetachCondAndBuildRangeForIndex, DetachCondsForColumn, HasFullRange, Range, Ranges, ast,
    collate, mysql, types,
};

/// 验证闭/开区间字符串与 IsPointNonNullable 语义。
#[test]
fn range_string_and_point_semantics_match_go() {
    let point = int_range(1, 1, false, false);
    assert_eq!(point.String(), "[1,1]");
    assert!(point.IsPointNonNullable((*types::StrictContext).clone()));

    let open = int_range(1, 2, true, true);
    assert_eq!(open.String(), "(1,2)");
    assert!(!open.IsPointNonNullable((*types::StrictContext).clone()));
}

/// 验证 FullRange、HasFullRange 与 Clone/Equal 语义。
#[test]
fn full_range_and_clone_semantics_match_go() {
    let full = Range {
        LowVal: vec![types::MinNotNullDatum()],
        HighVal: vec![types::MaxValueDatum()],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Range::default()
    };
    assert!(full.IsFullRange(false));
    assert!(HasFullRange(&[full.Clone()], false));

    let cloned = full.Clone();
    assert!(full.Equal(Some(&cloned)));
    assert_eq!(Ranges(vec![full]).Range().len(), 1);
}

/// 验证 IntersectRange 保留开闭边界。
#[test]
fn range_intersection_preserves_open_bounds() {
    let left = int_range(1, 5, false, false);
    let right = int_range(3, 7, true, false);
    let (intersection, error) = left.IntersectRange((*types::StrictContext).clone(), &right);

    assert!(error.is_none());
    let intersection = intersection.expect("ranges overlap");
    assert_eq!(intersection.String(), "(3,5]");
}

/// 验证 DetachCondsForColumn 只拆出目标列条件。
#[test]
fn checker_detaches_only_the_requested_column() {
    let context = ranger_context();
    let int_type = field_type(mysql::TypeLonglong);
    let indexed = expression::Column::new(int_type.clone(), 1, 11, 0);
    let other = expression::Column::new(int_type, 2, 22, 1);

    let (access, remained) = DetachCondsForColumn(
        &context,
        vec![
            equality(&context, &indexed, 7),
            equality(&context, &other, 9),
        ],
        indexed,
    );

    assert_eq!(access.len(), 1);
    assert_eq!(remained.len(), 1);
}

/// 验证 EQ 与 OR(DNF) 路径都能构造正确的索引点范围。
#[test]
fn detacher_builds_eq_and_dnf_index_ranges() {
    let context = ranger_context();
    let column = expression::Column::new(field_type(mysql::TypeLonglong), 1, 11, 0);

    let equality_result = DetachCondAndBuildRangeForIndex(
        &context,
        vec![equality(&context, &column, 7)],
        vec![column.clone()],
        vec![types::UnspecifiedLength],
        0,
    )
    .expect("EQ range detachment succeeds");
    assert_eq!(equality_result.AccessConds.len(), 1);
    assert_eq!(equality_result.RemainedConds.len(), 0);
    assert_eq!(equality_result.Ranges.len(), 1);
    assert_eq!(equality_result.Ranges[0].String(), "[7,7]");

    let disjunction = expression::NewFunctionBase(
        context.ExprCtx.as_ref(),
        ast::LogicOr,
        field_type(mysql::TypeTiny),
        vec![
            equality(&context, &column, 1),
            equality(&context, &column, 3),
        ],
    )
    .expect("OR must be constructible");
    let dnf_result = DetachCondAndBuildRangeForIndex(
        &context,
        vec![disjunction],
        vec![column],
        vec![types::UnspecifiedLength],
        0,
    )
    .expect("DNF range detachment succeeds");
    assert!(dnf_result.IsDNFCond);
    assert_eq!(dnf_result.Ranges.len(), 2);
    assert_eq!(dnf_result.Ranges[0].String(), "[1,1]");
    assert_eq!(dnf_result.Ranges[1].String(), "[3,3]");
}
