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

// mathutil 数值工具单测：位数、绝对值边界、有限性、钳制、2 的幂与批次划分。

use super::{
    Abs, Clamp, Divide2Batches, IntBits, IsFinite, MaxInt, MaxUint, MinInt, NextPowerOfTwo,
    StrLenOfInt64Fast, StrLenOfUint64Fast,
};

/// 用 LCG 随机样本与边界值校验 `StrLenOfUint64Fast`。
#[test]
fn test_str_len_of_uint64_fast() {
    let mut seed = 1_u64;
    for _ in 0..1_000_000 {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        assert_eq!(StrLenOfUint64Fast(seed), seed.to_string().len());
    }

    let nums = [
        0,
        1,
        12,
        123,
        1_234,
        12_345,
        123_456,
        1_234_567,
        12_345_678,
        123_456_789,
        1_234_567_890,
        1_234_567_891,
        12_345_678_912,
        123_456_789_123,
        1_234_567_891_234,
        12_345_678_912_345,
        123_456_789_123_456,
        1_234_567_891_234_567,
        12_345_678_912_345_678,
        123_456_789_123_456_789,
        123_456_789_123_457_890,
        u64::MAX,
    ];
    for num in nums {
        assert_eq!(StrLenOfUint64Fast(num), num.to_string().len());
    }
}

/// 校验 Abs 边界（含 i64::MIN）及 IntBits/MinInt/MaxInt/MaxUint。
#[test]
fn test_abs_and_int_length_boundaries() {
    assert_eq!(Abs(0), 0);
    assert_eq!(Abs(-123), 123);
    assert_eq!(Abs(i64::MIN), i64::MIN);
    for num in [0, 1, -1, i64::MAX, i64::MIN] {
        assert_eq!(StrLenOfInt64Fast(num), num.to_string().len());
    }
    assert_eq!(IntBits, usize::BITS as usize);
    assert_eq!(
        (MinInt, MaxInt, MaxUint),
        (isize::MIN, isize::MAX, usize::MAX)
    );
}

/// 校验有限数与 NaN/Inf 的判定。
#[test]
fn test_is_finite() {
    assert!(IsFinite(0.0));
    assert!(IsFinite(f64::MAX));
    assert!(!IsFinite(f64::NAN));
    assert!(!IsFinite(f64::INFINITY));
    assert!(!IsFinite(f64::NEG_INFINITY));
}

/// 校验数值与字符串的闭区间钳制。
#[test]
fn test_clamp() {
    assert_eq!(Clamp(100, 1, 3), 3);
    assert_eq!(Clamp(2.0_f64, 1.0, 3.0), 2.0);
    assert_eq!(Clamp(0.0_f32, 1.0, 3.0), 1.0);
    assert_eq!(Clamp(0, 1, 1), 1);
    assert_eq!(Clamp(100, 1, 1), 1);
    assert_eq!(Clamp("aa", "ab", "xy"), "ab");
    assert_eq!(Clamp("yy", "ab", "xy"), "xy");
    assert_eq!(Clamp("ab", "ab", "ab"), "ab");
}

/// 校验向上取最近 2 的幂。
#[test]
fn test_next_power_of_two() {
    assert_eq!(NextPowerOfTwo(1), 1);
    assert_eq!(NextPowerOfTwo(3), 4);
    assert_eq!(NextPowerOfTwo(255), 256);
    assert_eq!(NextPowerOfTwo(1024), 1024);
    assert_eq!(NextPowerOfTwo(0xabcd1234), 0x1_0000_0000);
}

/// 校验批次划分：余数优先落在前部批次，与 Go 期望一致。
#[test]
fn test_divide_2_batches() {
    assert_eq!(Divide2Batches(0_i32, 1), Vec::<i32>::new());
    assert_eq!(Divide2Batches(1_i32, 1), vec![1]);
    assert_eq!(Divide2Batches(1_i32, 3), vec![1]);
    assert_eq!(Divide2Batches(2_i32, 2), vec![1, 1]);
    assert_eq!(Divide2Batches(2_i32, 10), vec![1, 1]);
    assert_eq!(Divide2Batches(10_i32, 1), vec![10]);
    assert_eq!(Divide2Batches(10_i32, 2), vec![5, 5]);
    assert_eq!(Divide2Batches(10_i32, 3), vec![4, 3, 3]);
    assert_eq!(Divide2Batches(10_i32, 4), vec![3, 3, 2, 2]);
    assert_eq!(Divide2Batches(10_i32, 5), vec![2, 2, 2, 2, 2]);
}
