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

// 整数与浮点通用数学工具：绝对值、位数、有限性、钳制、2 的幂与批次划分。
//
// 对应 Go `pkg/util/mathutil`；位宽/边界常量随架构变化。`Divide2Batches` 用于
// 把总量拆成若干正数份（如并行扫描批大小）。

use std::ops::{AddAssign, Div, Rem, SubAssign};

#[cfg(all(
    feature = "formal-crate",
    any(test, feature = "intest", feature = "enableassert")
))]
use crate::assert::Assert;
#[cfg(feature = "formal-crate")]
use crate::assert_common::AssertArg;
#[cfg(all(
    feature = "formal-crate",
    not(any(test, feature = "intest", feature = "enableassert"))
))]
use crate::no_assert::Assert;
#[cfg(all(
    not(feature = "formal-crate"),
    any(test, feature = "intest", feature = "enableassert")
))]
use crate::pkg::util::intest::assert::Assert;
#[cfg(not(feature = "formal-crate"))]
use crate::pkg::util::intest::assert_common::AssertArg;
#[cfg(all(
    not(feature = "formal-crate"),
    not(any(test, feature = "intest", feature = "enableassert"))
))]
use crate::pkg::util::intest::no_assert::Assert;

// Architecture and/or implementation specific integer limits and bit widths.
/// 当前平台 `isize` 最大值。
pub const MaxInt: isize = isize::MAX;
/// 当前平台 `isize` 最小值。
pub const MinInt: isize = isize::MIN;
/// 当前平台 `usize` 最大值。
pub const MaxUint: usize = usize::MAX;
/// 当前平台整数位宽（`usize::BITS`）。
pub const IntBits: usize = usize::BITS as usize;

// Abs implements the branch-free Go int64 absolute-value formula.
// wrapping_sub preserves Go's two's-complement result for i64::MIN.
/// 无分支计算 i64 绝对值；对 `i64::MIN` 与 Go 一致保留补码溢出结果。
pub fn Abs(n: i64) -> i64 {
    let y = n >> 63;
    (n ^ y).wrapping_sub(y)
}

// uintSizeTable avoids repeated division when calculating decimal digit counts.
/// 十进制位数查找表：下标为位数，值为该位数能表示的最大 u64。
static uintSizeTable: [u64; 21] = [
    0,
    9,
    99,
    999,
    9_999,
    99_999,
    999_999,
    9_999_999,
    99_999_999,
    999_999_999,
    9_999_999_999,
    99_999_999_999,
    999_999_999_999,
    9_999_999_999_999,
    99_999_999_999_999,
    999_999_999_999_999,
    9_999_999_999_999_999,
    99_999_999_999_999_999,
    999_999_999_999_999_999,
    9_999_999_999_999_999_999,
    u64::MAX,
];

// StrLenOfUint64Fast efficiently calculates the decimal length of a u64.
/// 用查找表快速计算 u64 十进制字符串长度。
pub fn StrLenOfUint64Fast(x: u64) -> usize {
    for (length, limit) in uintSizeTable.iter().enumerate().skip(1) {
        if x <= *limit {
            return length;
        }
    }
    unreachable!("u64::MAX is the final size-table entry")
}

// StrLenOfInt64Fast includes the minus sign for negative values.
/// 计算 i64 十进制字符串长度（负数计入负号）。
pub fn StrLenOfInt64Fast(x: i64) -> usize {
    usize::from(x < 0) + StrLenOfUint64Fast(Abs(x) as u64)
}

// IsFinite reports whether f is neither NaN nor an infinity.
/// 判断 f64 是否为有限值（非 NaN、非 ±Inf）。
pub fn IsFinite(f: f64) -> bool {
    !(f - f).is_nan()
}

// Clamp restricts a value to a closed interval.
/// 将值钳制到闭区间 `[minv, maxv]`。
pub fn Clamp<T>(n: T, minv: T, maxv: T) -> T
where
    T: PartialOrd,
{
    if n >= maxv {
        maxv
    } else if n <= minv {
        minv
    } else {
        n
    }
}

// NextPowerOfTwo returns the smallest power of two greater than or equal to i.
// The caller must guarantee i > 0 and that the result does not overflow.
/// 返回不小于 `i` 的最小 2 的幂；调用方须保证 `i > 0` 且结果不溢出。
pub fn NextPowerOfTwo(mut i: i64) -> i64 {
    if i & (i - 1) == 0 {
        return i;
    }
    // 先放大再清除最低置位，直到只剩单一比特。
    i *= 2;
    while i & (i - 1) != 0 {
        i &= i - 1;
    }
    i
}

// Divide2Batches divides total into positive parts whose sum is total.
// If total < batches, it returns total parts of size 1. Batches must be > 0.
/// 将 `total` 拆成若干正数份，份数尽量接近 `batches`，各份之和等于 `total`。
/// `total < batches` 时返回 `total` 个 1；`batches` 必须 > 0。
pub fn Divide2Batches<T>(mut total: T, batches: T) -> Vec<T>
where
    T: Copy + Default + PartialOrd + AddAssign + SubAssign + Div<Output = T> + Rem<Output = T>,
{
    let zero = T::default();
    let quotient = total / batches;
    let mut remainder = total % batches;
    // batches must be positive, so this yields one for every integer type,
    // including i8 which cannot implement From<u8>.
    let one = batches / batches;
    let mut result = Vec::new();
    while total > zero {
        let mut size = quotient;
        if remainder > zero {
            // Distribute the remainder over the leading batches, matching Go.
            // 余数优先分给前面的批次，与 Go 行为一致。
            size += one;
            remainder -= one;
        }
        Assert(size > zero, &[AssertArg::from("size should be positive")]);
        result.push(size);
        total -= size;
    }
    result
}
