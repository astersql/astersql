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

// Range / Ranges 基础行为的单元测试。
//
// 覆盖：点范围判定与字符串展示、full range（含无符号 handle）、内存估算、
// 多列/单列区间交集（空集、子集、重叠）以及交集后粒度与 collator 数量保持。

use crate::test_support_aster_unit_test::int_range;
use crate::{EmptyRangeSize, Range, Ranges, collate, types};

/// 由整型上下界构造 Range；`i64::MIN`/`MAX` 映射为 ±inf 哨兵 Datum。
fn build_range(
    low_vals: &[i64],
    high_vals: &[i64],
    low_exclude: bool,
    high_exclude: bool,
) -> Range {
    let datums = |values: &[i64]| {
        values
            .iter()
            .map(|value| match *value {
                i64::MIN => types::MinNotNullDatum(),
                i64::MAX => types::MaxValueDatum(),
                value => types::NewIntDatum(value),
            })
            .collect()
    };
    Range {
        LowVal: datums(low_vals),
        HighVal: datums(high_vals),
        LowExclude: low_exclude,
        HighExclude: high_exclude,
        Collators: collate::GetBinaryCollatorSlice(low_vals.len().max(high_vals.len())),
    }
}

/// 计算两 Range 交集，并断言比较过程无错误。
fn intersection(left: &Range, right: &Range) -> Option<Range> {
    let (result, error) = left.IntersectRange((*types::StrictContext).clone(), right);
    assert!(error.is_none());
    result
}

/// 验证 Range 的 String 展示与 IsPointNonNullable 点范围判定。
#[test]
fn test_range() {
    let cases = [
        (int_range(1, 1, false, false), "[1,1]", true),
        (int_range(1, 1, false, true), "[1,1)", false),
        (int_range(1, 2, true, true), "(1,2)", false),
        (
            build_range(&[i64::MIN], &[1], false, true),
            "[-inf,1)",
            false,
        ),
    ];
    for (range, expected, is_point) in cases {
        assert_eq!(range.String(), expected);
        assert_eq!(
            range.IsPointNonNullable((*types::StrictContext).clone()),
            is_point
        );
    }

    let string_point = Range {
        LowVal: vec![types::NewStringDatum("abc".to_owned())],
        HighVal: vec![types::NewStringDatum("abc".to_owned())],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Range::default()
    };
    assert!(string_point.IsPointNonNullable((*types::StrictContext).clone()));

    let mismatched = Range {
        LowVal: vec![types::NewIntDatum(1)],
        HighVal: vec![types::NewIntDatum(1), types::NewIntDatum(1)],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Range::default()
    };
    assert!(!mismatched.IsPointNonNullable((*types::StrictContext).clone()));

    let float_range = Range {
        LowVal: vec![types::NewFloat64Datum(1.1)],
        HighVal: vec![types::NewFloat64Datum(1.9)],
        HighExclude: true,
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Range::default()
    };
    assert_eq!(float_range.String(), "[1.1,1.9)");
}

/// Go's `%v` formatting uses the user-facing ENUM/SET values rather than the
/// implementation's debug representation.
#[test]
fn test_range_string_formats_mysql_named_values() {
    let enum_datum = types::NewMysqlEnumDatum(types::Enum {
        Name: "red".to_owned(),
        Value: 1,
    });
    let set_datum = types::NewMysqlSetDatum(
        types::Set {
            Name: "read,write".to_owned(),
            Value: 3,
        },
        String::new(),
    );
    let range = Range {
        LowVal: vec![enum_datum.clone(), set_datum.clone()],
        HighVal: vec![enum_datum, set_datum],
        Collators: collate::GetBinaryCollatorSlice(2),
        ..Range::default()
    };

    assert_eq!(
        range.String(),
        "[\"red\" \"read,write\",\"red\" \"read,write\"]"
    );
}

/// 验证有符号/无符号 handle 下的 IsFullRange 判定。
#[test]
fn test_is_full_range() {
    let full = Range {
        LowVal: vec![types::MinNotNullDatum()],
        HighVal: vec![types::MaxValueDatum()],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Range::default()
    };
    assert!(full.IsFullRange(false));

    let unsigned = Range {
        LowVal: vec![types::NewUintDatum(0)],
        HighVal: vec![types::NewUintDatum(u64::MAX)],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Range::default()
    };
    assert!(unsigned.IsFullRange(true));
    assert!(!int_range(1, 9, false, false).IsFullRange(false));

    assert!(
        !Range {
            LowVal: vec![types::MaxValueDatum()],
            HighVal: vec![types::MinNotNullDatum()],
            Collators: collate::GetBinaryCollatorSlice(1),
            ..Range::default()
        }
        .IsFullRange(false)
    );

    let mut null = types::MinNotNullDatum();
    null.SetNull();
    assert!(
        Range {
            LowVal: vec![null.clone()],
            HighVal: vec![types::NewUintDatum(u64::MAX)],
            Collators: collate::GetBinaryCollatorSlice(1),
            ..Range::default()
        }
        .IsFullRange(false)
    );
    assert!(
        !Range {
            LowVal: vec![null.clone()],
            HighVal: vec![null],
            Collators: collate::GetBinaryCollatorSlice(1),
            ..Range::default()
        }
        .IsFullRange(false)
    );
}

/// 验证单 Range 与 Ranges 列表的 MemUsage 估算。
#[test]
fn test_range_mem_usage() {
    let integers = int_range(0, 1, false, false);
    let expected = EmptyRangeSize + 2 * types::EmptyDatumSize + 16;
    assert_eq!(integers.MemUsage(), expected);

    let strings = Range {
        LowVal: vec![types::NewStringDatum("abcde".to_owned())],
        HighVal: vec![types::NewStringDatum("fghij".to_owned())],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Range::default()
    };
    assert!(strings.MemUsage() > integers.MemUsage());
    assert_eq!(
        Ranges(vec![integers.clone(), strings.clone()]).MemUsage(),
        integers.MemUsage() + strings.MemUsage()
    );
}

/// 验证 Ranges.IntersectRanges 的多段两两交集结果。
#[test]
fn test_intersection_list() {
    let left = Ranges(vec![
        build_range(&[100, 0], &[100, i64::MAX], true, false),
        build_range(&[100], &[i64::MAX], true, false),
    ]);
    let right = Ranges(vec![
        build_range(&[i64::MIN], &[101], false, true),
        build_range(&[101, i64::MIN], &[101, 10], false, true),
    ]);
    let result = left
        .IntersectRanges((*types::StrictContext).clone(), right)
        .expect("matching collators");
    let strings: Vec<_> = result.iter().map(Range::String).collect();
    assert_eq!(
        strings,
        ["(100 0,100 +inf]", "(100,101)", "[101 -inf,101 10)"]
    );
}

/// 验证不相交或边界互斥时交集为空。
#[test]
fn test_intersection_empty() {
    let cases = [
        (int_range(1, 2, false, false), int_range(3, 4, false, false)),
        (int_range(1, 2, true, false), int_range(3, 4, true, false)),
        (int_range(1, 2, false, true), int_range(3, 4, false, true)),
        (int_range(1, 2, true, true), int_range(3, 4, true, true)),
        (
            build_range(&[1, 2], &[1, 3], false, false),
            build_range(&[1, 3], &[1, 4], true, false),
        ),
        (
            build_range(&[i64::MIN], &[1], false, false),
            build_range(&[2], &[i64::MAX], false, false),
        ),
        (
            build_range(&[1, 1, 2], &[1, 1, 5], false, false),
            build_range(&[1, 2], &[1, 3], true, true),
        ),
        (
            build_range(&[100, 0], &[100, i64::MAX], true, false),
            build_range(&[i64::MIN, i64::MIN], &[100, i64::MIN], false, false),
        ),
        (
            build_range(&[100, 0], &[100, i64::MAX], true, false),
            build_range(&[i64::MIN], &[100], false, true),
        ),
        (int_range(5, 5, false, false), int_range(5, 9, true, false)),
        (int_range(1, 1, false, false), int_range(5, 9, true, false)),
        (
            build_range(&[1], &[1], false, false),
            build_range(&[5, 1], &[5, i64::MAX], true, false),
        ),
    ];
    for (left, right) in cases {
        assert!(intersection(&left, &right).is_none());
        assert!(intersection(&right, &left).is_none());
    }
}

/// 验证内层区间完全落在外层时，交集等于内层（含开闭区间）。
#[test]
fn test_intersection_subset() {
    let cases = [
        (
            int_range(1, 5, false, false),
            int_range(2, 4, false, false),
            "[2,4]",
        ),
        (
            int_range(1, 5, true, false),
            int_range(2, 4, true, false),
            "(2,4]",
        ),
        (
            int_range(1, 5, false, true),
            int_range(2, 4, false, true),
            "[2,4)",
        ),
        (
            int_range(1, 5, true, true),
            int_range(2, 4, true, true),
            "(2,4)",
        ),
        (
            build_range(&[i64::MIN], &[5], false, false),
            build_range(&[2], &[4], false, false),
            "[2,4]",
        ),
        (
            build_range(&[1, 1, i64::MIN], &[1, 1, 15], false, false),
            build_range(&[1, 1], &[1, 1], false, false),
            "[1 1 -inf,1 1 15]",
        ),
        (
            build_range(&[1, 2], &[1, 3], false, false),
            build_range(&[1, 3], &[1, 4], false, false),
            "[1 3,1 3]",
        ),
    ];
    for (outer, inner, expected) in cases {
        assert_eq!(intersection(&outer, &inner).unwrap().String(), expected);
        assert_eq!(intersection(&inner, &outer).unwrap().String(), expected);
    }
}

/// 验证部分重叠时取公共区间，交换左右结果一致。
#[test]
fn test_intersection_overlap() {
    let cases = [
        (
            int_range(1, 5, false, false),
            int_range(2, 7, false, false),
            "[2,5]",
        ),
        (
            int_range(1, 5, true, false),
            int_range(2, 7, true, false),
            "(2,5]",
        ),
        (
            int_range(1, 5, false, true),
            int_range(2, 7, false, true),
            "[2,5)",
        ),
        (
            int_range(1, 5, true, true),
            int_range(2, 7, true, true),
            "(2,5)",
        ),
        (
            build_range(&[i64::MIN], &[5], false, false),
            build_range(&[2], &[14], false, false),
            "[2,5]",
        ),
        (
            build_range(&[1, 1, i64::MIN], &[1, 1, 15], false, false),
            build_range(&[1, 1, 4], &[1, 1, 25], false, false),
            "[1 1 4,1 1 15]",
        ),
        (
            build_range(&[5], &[5], false, false),
            build_range(&[5, 1], &[5, i64::MAX], true, false),
            "(5 1,5 +inf]",
        ),
    ];
    for (left, right, expected) in cases {
        assert_eq!(intersection(&left, &right).unwrap().String(), expected);
        assert_eq!(intersection(&right, &left).unwrap().String(), expected);
    }
}

/// 验证多列交集保留更细粒度边界，且 collator 数量与列宽一致。
#[test]
fn test_multicolumn_intersection_keeps_granularity() {
    let outer = build_range(&[1, 2], &[1, 5], false, false);
    let inner = build_range(&[1, 3], &[1, 4], true, false);
    let result = intersection(&outer, &inner).unwrap();
    assert_eq!(result.String(), "(1 3,1 4]");
    assert_eq!(result.Collators.len(), 2);
}
