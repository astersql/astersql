// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// mathutil 迁移补充单元测试。
//
// 对齐 Go 侧指数滑动平均、整数辅助函数、有限性/钳制、批次切分，
// 以及 MySQL 兼容伪随机数（MysqlRng）的种子序列与会话态访问器行为。

#[cfg(feature = "formal-crate")]
use crate::{
    Abs, Clamp, Divide2Batches, IsFinite, NewExponentialMovingAverage, NewWithSeed, NextPowerOfTwo,
    StrLenOfInt64Fast, StrLenOfUint64Fast,
};
#[cfg(not(feature = "formal-crate"))]
use astersql_util_mathutil::{
    Abs, Clamp, Divide2Batches, IsFinite, NewExponentialMovingAverage, NewWithSeed, NextPowerOfTwo,
    StrLenOfInt64Fast, StrLenOfUint64Fast,
};

/// 验证指数滑动平均预热与衰减数值与 Go 一致。
#[test]
fn exponential_average_matches_go_warmup_and_decay() {
    let mut average = NewExponentialMovingAverage(0.8, 2);
    average.Add(10.0);
    assert_eq!(average.Get(), 10.0);
    average.Add(20.0);
    assert_eq!(average.Get(), 15.0);
    average.Add(30.0);
    assert_eq!(average.Get(), 27.0);
}

/// factor 必须落在 (0, 1)；非法参数应按 Go 同样 panic。
#[test]
#[should_panic(expected = "factor must be (0, 1)")]
fn exponential_average_rejects_invalid_factor_like_go() {
    let _ = NewExponentialMovingAverage(1.0, 2);
}

/// 覆盖绝对值、十进制位数、下一 2 次幂等边界样例。
#[test]
fn integer_helpers_cover_go_boundaries() {
    assert_eq!(Abs(i64::MIN), i64::MIN);
    assert_eq!(StrLenOfUint64Fast(0), 1);
    assert_eq!(StrLenOfUint64Fast(u64::MAX), 20);
    assert_eq!(StrLenOfInt64Fast(i64::MIN), 20);
    assert_eq!(NextPowerOfTwo(0xabcd_1234), 0x1_0000_0000);
}

/// IsFinite / Clamp 对无穷、NaN 与区间边界的行为应对齐 Go。
#[test]
fn finite_and_clamp_match_go_comparisons() {
    assert!(IsFinite(1.25));
    assert!(!IsFinite(f64::INFINITY));
    assert!(!IsFinite(f64::NEG_INFINITY));
    assert!(!IsFinite(f64::NAN));
    assert_eq!(Clamp(100, 1, 3), 3);
    assert_eq!(Clamp("aa", "ab", "xy"), "ab");
}

/// Divide2Batches 按批大小均分总量，支持有符号/无符号整数宽度。
#[test]
fn divide_batches_supports_all_go_integer_widths() {
    let small_signed: Vec<i8> = Divide2Batches(10_i8, 4_i8);
    assert_eq!(small_signed, vec![3, 3, 2, 2]);
    let unsigned: Vec<u64> = Divide2Batches(2_u64, 10_u64);
    assert_eq!(unsigned, vec![1, 1]);
    assert!(Divide2Batches(0_i32, 1_i32).is_empty());
}

/// 固定种子下连续 Gen() 输出应与 Go MysqlRng 黄金值一致。
#[test]
fn mysql_rng_matches_go_seed_sequences() {
    let cases = [
        (0, 0.15522042769493574, 0.620881741513388),
        (1, 0.40540353712197724, 0.8716141803857071),
        (-1, 0.9050373219931845, 0.37014932126752037),
        (i64::MAX, 0.9050373219931845, 0.37014932126752037),
    ];
    for (seed, first, second) in cases {
        let rng = NewWithSeed(seed);
        assert_eq!(rng.Gen(), first);
        assert_eq!(rng.Gen(), second);
    }
}

/// SetSeed1/SetSeed2 模拟会话态恢复后，序列与 GetSeed 结果应对齐 Go。
#[test]
fn mysql_rng_seed_accessors_match_go_session_state() {
    let rng = NewWithSeed(0);
    rng.SetSeed1(10_000_000);
    rng.SetSeed2(1_000_000);
    assert_eq!(rng.Gen(), 0.028870999839968048);
    assert_eq!(rng.Gen(), 0.11641535266900002);
    assert_eq!(rng.Gen(), 0.49546379455874096);
    assert_eq!(rng.GetSeed1(), 532_000_198);
    assert_eq!(rng.GetSeed2(), 689_000_330);
}
