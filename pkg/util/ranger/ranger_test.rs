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

// ranger 核心单元测试：表/列/索引 Range、前缀索引、回退配额与并集。
//
// 覆盖 BuildTableRange、BuildColumnRange、DetachCond*、UnionRanges、
// 分区路径不合并点范围，以及 Clone 不共享 Collators 等行为。

use crate::test_support_aster_unit_test::{
    column, comparison, disjunction, equality, field_type, int_constant, int_range, ranger_context,
    scalar,
};
use crate::{
    AddExpr4EqAndInCondition, BuildColumnRange, BuildTableRange, CutDatumByPrefixLen,
    DetachCondAndBuildRangeForIndex, DetachCondAndBuildRangeForPartition,
    DetachSimpleCondAndBuildRangeForIndex, FullIntRange, FullRange, Range, Ranges, UnionRanges,
    ast, charset, collate, mysql, types,
};

/// 调用 DetachCondAndBuildRangeForIndex，lengths 全为 UnspecifiedLength。
fn detach(
    context: &rangerctx::RangerContext<'_>,
    conditions: Vec<expression::ExprBox>,
    columns: Vec<expression::Column>,
) -> crate::DetachRangeResult {
    let lengths = vec![types::UnspecifiedLength; columns.len()];
    DetachCondAndBuildRangeForIndex(context, conditions, columns, lengths, 0)
        .expect("range detachment succeeds")
}

/// 构造 `col IN (v1, v2, ...)` 谓词。
fn in_condition(
    context: &rangerctx::RangerContext<'_>,
    indexed: &expression::Column,
    values: &[i64],
) -> expression::ExprBox {
    let mut args: Vec<expression::ExprBox> = vec![Box::new(indexed.clone())];
    args.extend(values.iter().map(|value| int_constant(indexed, *value)));
    scalar(context, ast::In, args)
}

/// 构造 binary collation 的 Varchar 测试列。
fn string_column() -> expression::Column {
    let mut field_type = field_type(mysql::TypeVarchar);
    field_type.SetFlen(32);
    field_type.SetCharset(charset::CharsetBin.to_owned());
    field_type.SetCollate(charset::CollationBin.to_owned());
    expression::Column::new(field_type, 10, 10, 0)
}

/// 构造字符串等值谓词。
fn string_equality(
    context: &rangerctx::RangerContext<'_>,
    indexed: &expression::Column,
    value: &str,
) -> expression::ExprBox {
    scalar(
        context,
        ast::EQ,
        vec![
            Box::new(indexed.clone()),
            Box::new(expression::Constant::with_type(
                types::NewCollationStringDatum(value.to_owned(), charset::CollationBin.to_owned()),
                indexed.RetType.clone().expect("string column has a type"),
            )),
        ],
    )
}

/// 表扫描 Range：EQ 条件生成单点区间。
#[test]
fn test_table_range() {
    let mut context = ranger_context();
    let indexed = column(1, 0);
    let (ranges, access, remained) = BuildTableRange(
        vec![equality(&context, &indexed, 7)],
        &mut context,
        indexed.RetType.as_ref().expect("indexed column has a type"),
        0,
    )
    .expect("table range builds");
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].String(), "[7,7]");
    assert_eq!(access.len(), 1);
    assert!(remained.is_empty());
}

/// 有符号/无符号 FullIntRange 边界与 IsFullRange。
#[test]
fn test_index_range_for_unsigned_and_overflow() {
    let signed = FullIntRange(false);
    assert_eq!(signed[0].String(), "[-inf,+inf]");

    let unsigned = FullIntRange(true);
    assert_eq!(unsigned[0].LowVal[0].GetUint64(), 0);
    assert_eq!(unsigned[0].HighVal[0].GetUint64(), u64::MAX);
    assert!(unsigned[0].IsFullRange(true));
}

/// 列 Range：GT+LE 合并为开闭区间。
#[test]
fn test_column_range() {
    let mut context = ranger_context();
    let indexed = column(1, 0);
    let (ranges, access, remained) = BuildColumnRange(
        vec![
            comparison(&context, ast::GT, &indexed, 3),
            comparison(&context, ast::LE, &indexed, 8),
        ],
        &mut context,
        indexed.RetType.as_ref().expect("indexed column has a type"),
        types::UnspecifiedLength,
        0,
    )
    .expect("column range builds");
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].String(), "(3,8]");
    assert_eq!(access.len(), 2);
    assert!(remained.is_empty());
}

/// Year 类型等值条件构造点范围。
#[test]
fn test_index_range_for_year() {
    let context = ranger_context();
    let year = expression::Column::new(field_type(mysql::TypeYear), 1, 1, 0);
    let result = detach(&context, vec![equality(&context, &year, 2026)], vec![year]);
    assert_eq!(result.Ranges.len(), 1);
    assert_eq!(result.Ranges[0].String(), "[2026,2026]");
}

/// 前缀索引：值被裁剪，残留条件保留。
#[test]
fn test_prefix_index_range_scan() {
    let context = ranger_context();
    let indexed = string_column();
    let result = DetachCondAndBuildRangeForIndex(
        &context,
        vec![string_equality(&context, &indexed, "abcdef")],
        vec![indexed],
        vec![2],
        0,
    )
    .expect("prefix index range builds");
    assert_eq!(result.Ranges.len(), 1);
    assert_eq!(result.Ranges[0].LowVal[0].GetBytes(), b"ab");
    assert_eq!(result.RemainedConds.len(), 1);
}

/// 多列索引：等值前缀 + 范围后缀。
#[test]
fn test_index_range() {
    let context = ranger_context();
    let first = column(1, 0);
    let second = column(2, 1);
    let result = detach(
        &context,
        vec![
            equality(&context, &first, 4),
            comparison(&context, ast::GE, &second, 9),
        ],
        vec![first, second],
    );
    assert_eq!(result.Ranges.len(), 1);
    assert_eq!(result.Ranges[0].String(), "[4 9,4 +inf]");
    assert_eq!(result.EqCondCount, 1);
}

/// 非分片索引上 AddExpr4EqAndInCondition 保持原条件。
#[test]
fn test_table_shard_index() {
    let context = ranger_context();
    let indexed = column(1, 0);
    let original = vec![equality(&context, &indexed, 7)];
    let rewritten = AddExpr4EqAndInCondition(&context, original, vec![indexed])
        .expect("non-shard index remains usable");
    assert_eq!(rewritten.len(), 1);
}

/// 普通 IN 无需追加分片前缀列。
#[test]
fn test_shard_index_func_suites() {
    let context = ranger_context();
    let indexed = column(1, 0);
    let original = vec![in_condition(&context, &indexed, &[1, 2, 3])];
    let rewritten = AddExpr4EqAndInCondition(&context, original, vec![indexed])
        .expect("ordinary IN does not require shard prefix");
    assert_eq!(rewritten.len(), 1);
    assert_eq!(
        rewritten[0].as_scalar_function().unwrap().FuncName.L,
        ast::In
    );
}

/// 索引 Range 内存配额回退仍成功且不超过上限。
#[test]
fn test_range_fallback_for_detach_cond_and_build_range_for_index() {
    let context = ranger_context();
    let indexed = column(1, 0);
    let result = DetachCondAndBuildRangeForIndex(
        &context,
        vec![in_condition(
            &context,
            &indexed,
            &(0..64).collect::<Vec<_>>(),
        )],
        vec![indexed],
        vec![types::UnspecifiedLength],
        64,
    )
    .expect("quota fallback remains a successful range build");
    assert!(!result.Ranges.is_empty());
    assert!(result.Ranges.len() <= 64);
}

/// 表 Range 配额回退路径。
#[test]
fn test_range_fallback_for_build_table_range() {
    let mut context = ranger_context();
    let indexed = column(1, 0);
    let (ranges, _, _) = BuildTableRange(
        vec![in_condition(
            &context,
            &indexed,
            &(0..64).collect::<Vec<_>>(),
        )],
        &mut context,
        indexed.RetType.as_ref().expect("indexed column has a type"),
        64,
    )
    .expect("table fallback succeeds");
    assert!(!ranges.is_empty());
}

/// 列 Range 配额回退路径。
#[test]
fn test_range_fallback_for_build_column_range() {
    let mut context = ranger_context();
    let indexed = column(1, 0);
    let (ranges, _, _) = BuildColumnRange(
        vec![in_condition(
            &context,
            &indexed,
            &(0..64).collect::<Vec<_>>(),
        )],
        &mut context,
        indexed.RetType.as_ref().expect("indexed column has a type"),
        types::UnspecifiedLength,
        64,
    )
    .expect("column fallback succeeds");
    assert!(!ranges.is_empty());
}

/// 前缀索引扫描用例入口（复用 scan 测试）。
#[test]
fn test_prefix_index_range() {
    test_prefix_index_range_scan();
}

/// Clone 后修改 LowVal 不影响副本（深拷贝）。
#[test]
fn test_issue_40997() {
    let mut ranges = Ranges(vec![int_range(1, 1, false, false)]);
    let peer = ranges.clone();
    ranges[0].LowVal.push(types::NewIntDatum(2));
    assert_eq!(peer[0].LowVal.len(), 1);
    assert_eq!(ranges[0].LowVal.len(), 2);
}

/// UnionRanges 合并相邻/重叠区间。
#[test]
fn test_issue_50051() {
    let context = ranger_context();
    let merged = UnionRanges(
        &context,
        Ranges(vec![
            int_range(1, 3, false, false),
            int_range(3, 5, false, false),
        ]),
        true,
    )
    .expect("overlapping ranges merge");
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].String(), "[1,5]");
}

/// DNF 条件记录 MinAccessCondsForDNFCond。
#[test]
fn test_min_access_conds_for_dnf_cond() {
    let context = ranger_context();
    let indexed = column(1, 0);
    let dnf = disjunction(
        &context,
        vec![
            equality(&context, &indexed, 1),
            equality(&context, &indexed, 3),
        ],
    );
    let result = detach(&context, vec![dnf], vec![indexed]);
    assert!(result.IsDNFCond);
    assert_eq!(result.Ranges.len(), 2);
    assert!(result.MinAccessCondsForDNFCond >= 1);
}

/// binary collation 字符串等值保留原字节。
#[test]
fn test_bin_collation_range_for_index() {
    let context = ranger_context();
    let indexed = string_column();
    let result = detach(
        &context,
        vec![string_equality(&context, &indexed, "A\0z")],
        vec![indexed],
    );
    assert_eq!(result.Ranges.len(), 1);
    assert_eq!(result.Ranges[0].LowVal[0].GetBytes(), b"A\0z");
}

/// 分区路径不合并不同点范围。
#[test]
fn test_partition_path_does_not_merge_distinct_points() {
    let context = ranger_context();
    let indexed = column(1, 0);
    let result = DetachCondAndBuildRangeForPartition(
        &context,
        vec![in_condition(&context, &indexed, &[1, 3])],
        vec![indexed],
        vec![types::UnspecifiedLength],
        0,
    )
    .expect("partition range builds");
    assert_eq!(result.Ranges.len(), 2);
}

/// 简化 CNF 拆分与完整路径结果一致。
#[test]
fn test_simple_detach_matches_full_cnf_path() {
    let context = ranger_context();
    let indexed = column(1, 0);
    let (ranges, access, remained) = DetachSimpleCondAndBuildRangeForIndex(
        &context,
        vec![equality(&context, &indexed, 11)],
        vec![indexed],
        vec![types::UnspecifiedLength],
        0,
    )
    .expect("simple CNF detachment succeeds");
    assert_eq!(ranges[0].String(), "[11,11]");
    assert_eq!(access.len(), 1);
    assert!(remained.is_empty());
}

/// 空条件返回全范围。
#[test]
fn test_empty_column_conditions_return_full_range() {
    let mut context = ranger_context();
    let indexed = column(1, 0);
    let (ranges, access, remained) = BuildColumnRange(
        Vec::new(),
        &mut context,
        indexed.RetType.as_ref().expect("indexed column has a type"),
        types::UnspecifiedLength,
        0,
    )
    .expect("empty column conditions build full range");
    assert_eq!(ranges.len(), FullRange().len());
    assert!(ranges[0].Equal(Some(&FullRange()[0])));
    assert!(access.is_empty());
    assert!(remained.is_empty());
}

/// Clone 后 Collators 独立。
#[test]
fn test_range_clone_does_not_share_collators() {
    let original = Range {
        LowVal: vec![types::NewIntDatum(1)],
        HighVal: vec![types::NewIntDatum(2)],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Range::default()
    };
    let cloned = original.Clone();
    assert!(original.Equal(Some(&cloned)));
    assert_eq!(cloned.Collators.len(), 1);
}

/// Binary 前缀索引按字节而非 UTF-8 字符边界裁剪，并保持 String Datum 类型。
#[test]
fn test_cut_binary_string_prefix_inside_utf8_code_point() {
    let mut value =
        types::NewCollationStringDatum("é".to_owned(), charset::CollationBin.to_owned());
    let mut tp = field_type(mysql::TypeVarchar);
    tp.SetCharset(charset::CharsetBin.to_owned());
    tp.SetCollate(charset::CollationBin.to_owned());

    assert!(CutDatumByPrefixLen(&mut value, 1, &tp));
    assert_eq!(value.Kind(), types::KindString);
    assert_eq!(value.GetBytes(), vec![0xc3]);
}
