// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Datum / 向量比较单测，对齐 Go `types` 包 Compare 语义。
//
// 覆盖跨类型 Datum 比较、边界 Datum（Max/Min/Null）以及
// 有符号/无符号整数向量比较（VecCompare*）。

use std::any::Any;

use crate::datum::*;
use crate::time::FromGoTime;
use chrono::Utc;
use parser_types::mysql;
use types_decimal::mydecimal::NewDecFromInt;
use types_group_1::{DefaultStmtFlags, VecCompareII, VecCompareIU, VecCompareUI, VecCompareUU};

/// 在忽略截断错误的 Context 下用 Binary Collator 比较两个值。
fn compare_for_test(left: &dyn Any, right: &dyn Any) -> Result<i32, crate::datum::errors::Error> {
    let context = DefaultStmtNoWarningContext
        .clone()
        .WithFlags(DefaultStmtFlags.WithIgnoreTruncateErr(true));
    let left = NewDatum(left);
    let right = NewDatum(right);
    let collator = collate::GetBinaryCollator();
    left.Compare(context, &right, collator.as_ref())
}

/// 断言正向比较结果，并校验反向比较为相反数。
fn assert_compare<L: Any, R: Any>(index: usize, left: L, right: R, expected: i32) {
    let actual = compare_for_test(&left, &right).unwrap();
    assert_eq!(actual, expected, "case {index} forward");
    let reverse = compare_for_test(&right, &left).unwrap();
    assert_eq!(reverse, -expected, "case {index} reverse");
}

/// 表驱动覆盖数值、字符串、时间、二进制字面量、Enum/Set 等跨类型比较。
#[test]
fn test_compare() {
    let now = Utc::now().with_timezone(&chrono_tz::UTC);
    let later = now + chrono::Duration::seconds(10);
    let mut index = 0usize;

    macro_rules! case {
        ($left:expr, $right:expr, $expected:expr) => {{
            assert_compare(index, $left, $right, $expected);
            index += 1;
        }};
    }

    case!(1_f64, 1_f64, 0);
    case!(1_f64, "1".to_owned(), 0);
    case!(1_i64, 1_i64, 0);
    case!(-1_i64, 1_u64, -1);
    case!(-1_i64, "-1".to_owned(), 0);
    case!(1_u64, 1_u64, 0);
    case!(1_u64, -1_i64, 1);
    case!(1_u64, "1".to_owned(), 0);
    case!(NewDecFromInt(1), NewDecFromInt(1), 0);
    case!(NewDecFromInt(1), "1".to_owned(), 0);
    case!(NewDecFromInt(1), b"1".to_vec(), 0);
    case!("1".to_owned(), "1".to_owned(), 0);
    case!("1".to_owned(), -1_i64, 1);
    case!("1".to_owned(), 2_f64, -1);
    case!("1".to_owned(), 1_u64, 0);
    case!("1".to_owned(), NewDecFromInt(1), 0);
    case!(
        "2011-01-01 11:11:11".to_owned(),
        NewTime(FromGoTime(now), mysql::TypeDatetime, 0),
        -1
    );
    case!("12:00:00".to_owned(), ZeroDuration, 1);
    case!(ZeroDuration, ZeroDuration, 0);
    case!(
        NewTime(FromGoTime(later), mysql::TypeDatetime, 0),
        NewTime(FromGoTime(now), mysql::TypeDatetime, 0),
        1
    );
    case!((), 2_i32, -1);
    case!((), (), 0);
    case!(false, (), 1);
    case!(false, true, -1);
    case!(true, true, 0);
    case!(false, false, 0);
    case!(true, 2_i32, -1);
    case!(1.23_f64, (), 1);
    case!(0.0_f64, 3.45_f64, -1);
    case!(354.23_f64, 3.45_f64, 1);
    case!(3.452_f64, 3.452_f64, 0);
    case!(432_i32, (), 1);
    case!(-4_i32, 32_i32, -1);
    case!(4_i32, -32_i32, 1);
    case!(432_i32, 12_i64, 1);
    case!(23_i32, 128_i64, -1);
    case!(123_i32, 123_i64, 0);
    case!(432_i32, 12_i32, 1);
    case!(23_i32, 123_i32, -1);
    case!(133_i64, 183_i32, -1);
    case!(133_u64, 183_u64, -1);
    case!(2_u64, -2_i64, 1);
    case!(2_u64, 1_i64, 1);
    case!(String::new(), (), 1);
    case!(String::new(), "24".to_owned(), -1);
    case!("aasf".to_owned(), "4".to_owned(), 1);
    case!(String::new(), String::new(), 0);
    case!(Vec::<u8>::new(), (), 1);
    case!(Vec::<u8>::new(), b"sff".to_vec(), -1);
    case!(NewTime(CoreTime::default(), 0, 0), (), 1);
    case!(
        NewTime(CoreTime::default(), 0, 0),
        NewTime(FromGoTime(now), mysql::TypeDatetime, 3),
        -1
    );
    case!(
        NewTime(FromGoTime(now), mysql::TypeDatetime, 3),
        "0000-00-00 00:00:00".to_owned(),
        1
    );
    case!(
        Duration {
            Duration: 34,
            Fsp: 2
        },
        (),
        1
    );
    case!(
        Duration {
            Duration: 34,
            Fsp: 2
        },
        Duration {
            Duration: 29_034,
            Fsp: 2
        },
        -1
    );
    case!(
        Duration {
            Duration: 3_340,
            Fsp: 2
        },
        Duration {
            Duration: 34,
            Fsp: 2
        },
        1
    );
    case!(
        Duration {
            Duration: 34,
            Fsp: 2
        },
        Duration {
            Duration: 34,
            Fsp: 2
        },
        0
    );
    case!(Vec::<u8>::new(), Vec::<u8>::new(), 0);
    case!(b"abc".to_vec(), b"ab".to_vec(), 1);
    case!(b"123".to_vec(), 1234_i32, -1);
    case!(Vec::<u8>::new(), (), 1);
    case!(NewBinaryLiteralFromUint(1, -1), 1_i32, 0);
    case!(
        NewBinaryLiteralFromUint(0x4D7953514C, -1),
        "MySQL".to_owned(),
        0
    );
    case!(NewBinaryLiteralFromUint(0, -1), 10_u64, -1);
    case!(NewBinaryLiteralFromUint(1, -1), 0_f64, 1);
    case!(NewBinaryLiteralFromUint(1, -1), NewDecFromInt(1), 0);
    case!(
        NewBinaryLiteralFromUint(1, -1),
        NewBinaryLiteralFromUint(0, -1),
        1
    );
    case!(
        NewBinaryLiteralFromUint(1, -1),
        NewBinaryLiteralFromUint(1, -1),
        0
    );
    case!(
        Enum {
            Name: "a".to_owned(),
            Value: 1
        },
        1_i32,
        0
    );
    case!(
        Enum {
            Name: "a".to_owned(),
            Value: 1
        },
        "a".to_owned(),
        0
    );
    case!(
        Enum {
            Name: "a".to_owned(),
            Value: 1
        },
        10_u64,
        -1
    );
    case!(
        Enum {
            Name: "a".to_owned(),
            Value: 1
        },
        0_f64,
        1
    );
    case!(
        Enum {
            Name: "a".to_owned(),
            Value: 1
        },
        NewDecFromInt(1),
        0
    );
    case!(
        Enum {
            Name: "a".to_owned(),
            Value: 1
        },
        NewBinaryLiteralFromUint(2, -1),
        -1
    );
    case!(
        Enum {
            Name: "a".to_owned(),
            Value: 1
        },
        NewBinaryLiteralFromUint(1, -1),
        0
    );
    case!(
        Enum {
            Name: "a".to_owned(),
            Value: 1
        },
        Enum {
            Name: "a".to_owned(),
            Value: 1
        },
        0
    );
    case!(
        Set {
            Name: "a".to_owned(),
            Value: 1
        },
        1_i32,
        0
    );
    case!(
        Set {
            Name: "a".to_owned(),
            Value: 1
        },
        "a".to_owned(),
        0
    );
    case!(
        Set {
            Name: "a".to_owned(),
            Value: 1
        },
        10_u64,
        -1
    );
    case!(
        Set {
            Name: "a".to_owned(),
            Value: 1
        },
        0_f64,
        1
    );
    case!(
        Set {
            Name: "a".to_owned(),
            Value: 1
        },
        NewDecFromInt(1),
        0
    );
    case!(
        Set {
            Name: "a".to_owned(),
            Value: 1
        },
        NewBinaryLiteralFromUint(2, -1),
        -1
    );
    case!(
        Set {
            Name: "a".to_owned(),
            Value: 1
        },
        NewBinaryLiteralFromUint(1, -1),
        0
    );
    case!(
        Set {
            Name: "a".to_owned(),
            Value: 1
        },
        Enum {
            Name: "a".to_owned(),
            Value: 1
        },
        0
    );
    case!(
        Set {
            Name: "a".to_owned(),
            Value: 1
        },
        Set {
            Name: "a".to_owned(),
            Value: 1
        },
        0
    );
    case!("hello".to_owned(), NewDecFromInt(0), 0);
    case!(NewDecFromInt(0), "hello".to_owned(), 0);

    assert_eq!(index, 86);
}

/// 校验 MaxValue / MinNotNull / 默认（NULL）等边界 Datum 的比较顺序。
#[test]
fn test_compare_datum() {
    let cases = [
        (MaxValueDatum(), NewDatum(&"00:00:00".to_owned()), 1),
        (MinNotNullDatum(), NewDatum(&"00:00:00".to_owned()), -1),
        (Datum::default(), NewDatum(&"00:00:00".to_owned()), -1),
        (Datum::default(), Datum::default(), 0),
        (MinNotNullDatum(), MinNotNullDatum(), 0),
        (MaxValueDatum(), MaxValueDatum(), 0),
        (Datum::default(), MinNotNullDatum(), -1),
        (MinNotNullDatum(), MaxValueDatum(), -1),
    ];
    let context = DefaultStmtNoWarningContext
        .clone()
        .WithFlags(DefaultStmtFlags.WithIgnoreTruncateErr(true));

    for (index, (left, right, expected)) in cases.iter().enumerate() {
        let collator = collate::GetBinaryCollator();
        assert_eq!(
            left.Compare(context.clone(), right, collator.as_ref())
                .unwrap(),
            *expected,
            "case {index} forward"
        );
        let collator = collate::GetBinaryCollator();
        assert_eq!(
            right
                .Compare(context.clone(), left, collator.as_ref())
                .unwrap(),
            -*expected,
            "case {index} reverse"
        );
    }
}

/// 逐元素断言向量比较结果。
fn assert_vector_result(actual: &[i64], expected: &[i64]) {
    assert_eq!(actual.len(), expected.len());
    for (index, value) in actual.iter().enumerate() {
        assert_eq!(*value, expected[index], "index={index}");
    }
}

/// 覆盖 UU/II/IU/UI 四类有符号与无符号整数向量比较。
#[test]
fn test_vec_compare_int_and_uint() {
    // 无符号对无符号
    let cmp_uu = [
        (
            [0_u64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [9_u64, 8, 7, 6, 5, 4, 3, 2, 1, 0],
            [-1_i64, -1, -1, -1, -1, 1, 1, 1, 1, 1],
        ),
        (
            [0_u64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [0_u64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [0_i64; 10],
        ),
        (
            std::array::from_fn(|index| i64::MAX as u64 + index as u64),
            std::array::from_fn(|index| i64::MAX as u64 + index as u64),
            [0_i64; 10],
        ),
    ];
    for (left, right, expected) in cmp_uu {
        let mut result = [0_i64; 10];
        VecCompareUU(&left, &right, &mut result);
        assert_vector_result(&result, &expected);
    }

    let cmp_ii = [
        (
            [0_i64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [9_i64, 8, 7, 6, 5, 4, 3, 2, 1, 0],
            [-1_i64, -1, -1, -1, -1, 1, 1, 1, 1, 1],
        ),
        (
            [0_i64, -1, -2, -3, -4, -5, -6, -7, -8, -9],
            [9_i64, 8, 7, 6, 5, 4, 3, 2, 1, 0],
            [-1_i64; 10],
        ),
        (
            [0_i64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [-9_i64, -8, -7, -6, -5, -4, -3, -2, -1, 0],
            [1_i64; 10],
        ),
        (
            [0_i64, -1, -2, -3, -4, -5, -6, -7, -8, -9],
            [-9_i64, -8, -7, -6, -5, -4, -3, -2, -1, 0],
            [1_i64, 1, 1, 1, 1, -1, -1, -1, -1, -1],
        ),
        (
            [0_i64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [0_i64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [0_i64; 10],
        ),
    ];
    for (left, right, expected) in cmp_ii {
        let mut result = [0_i64; 10];
        VecCompareII(&left, &right, &mut result);
        assert_vector_result(&result, &expected);
    }

    let cmp_iu = [
        (
            [0_i64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [9_u64, 8, 7, 6, 5, 4, 3, 2, 1, 0],
            [-1_i64, -1, -1, -1, -1, 1, 1, 1, 1, 1],
        ),
        (
            [0_i64, -1, -2, -3, -4, -5, -6, -7, -8, -9],
            [9_u64, 8, 7, 6, 5, 4, 3, 2, 1, 0],
            [-1_i64; 10],
        ),
        (
            [0_i64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [0_u64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [0_i64; 10],
        ),
        (
            [0_i64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [i64::MAX as u64 + 1; 10],
            [-1_i64; 10],
        ),
    ];
    for (left, right, expected) in cmp_iu {
        let mut result = [0_i64; 10];
        VecCompareIU(&left, &right, &mut result);
        assert_vector_result(&result, &expected);
    }

    let cmp_ui = [
        (
            [0_u64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [9_i64, 8, 7, 6, 5, 4, 3, 2, 1, 0],
            [-1_i64, -1, -1, -1, -1, 1, 1, 1, 1, 1],
        ),
        (
            [0_u64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [-9_i64, -8, -7, -6, -5, -4, -3, -2, -1, 0],
            [1_i64; 10],
        ),
        (
            [i64::MAX as u64 + 1; 10],
            [0_i64, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [1_i64; 10],
        ),
    ];
    for (left, right, expected) in cmp_ui {
        let mut result = [0_i64; 10];
        VecCompareUI(&left, &right, &mut result);
        assert_vector_result(&result, &expected);
    }
}
