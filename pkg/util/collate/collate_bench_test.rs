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

// collate 基准入口的 Rust 移植与冒烟测试。
//
// 稳定版 Rust 无内置 benchmark harness；以 N=1 逐一跑完 Go 同名 Compare/Key/ImmutableKey 矩阵。

// Port of pkg/util/collate/collate_bench_test.go.
// Stable Rust has no built-in benchmark harness; the smoke test runs each Go
// benchmark entry once (N=1) across short/middle/long inputs.

#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use rand::Rng;
use util_collate::*;

/// 最小 `testing.B` 替身：仅保留迭代次数与 ResetTimer 空操作。
mod testing {
    /// 对应 Go `testing.B` 的精简字段。
    #[derive(Clone, Copy)]
    pub struct B {
        /// 基准循环次数（冒烟测试固定为 1）。
        pub N: usize,
    }

    impl B {
        /// 构造给定迭代次数的 B。
        pub fn new(n: usize) -> Self {
            Self { N: n }
        }

        /// Go 计时重置占位；冒烟路径不采集墙钟。
        pub fn ResetTimer(&mut self) {}
    }
}

/// 短输入长度（与 Go short 一致）。
const short: usize = 2 << 4;
/// 中等输入长度。
const middle: usize = 2 << 10;
/// 长输入长度（与 Go long 一致）。
const long: usize = 2 << 20;

// generateData 对应 Go helper：从 "ßss" 的 rune 集合中随机抽取 length 个字符组成测试字符串。
fn generateData(length: usize) -> String {
    let rs = ['ß', 's', 's'];
    let mut r = Vec::with_capacity(length);
    let mut rng = rand::rng();
    for _ in 0..length {
        r.push(rs[rng.random_range(0..rs.len())]);
    }
    r.into_iter().collect()
}

// compare 对应 Go benchmark helper：ResetTimer 后重复调用 Collator.Compare。
fn compare(mut b: testing::B, collator: Box<dyn Collator>, length: usize) {
    let s1 = generateData(length);
    let s2 = generateData(length);
    b.ResetTimer();
    for _ in 0..b.N {
        std::hint::black_box(collator.Compare(&s1, &s2));
    }
}

// key 对应 Go benchmark helper：ResetTimer 后重复生成可变 collation key。
fn key(mut b: testing::B, collator: Box<dyn Collator>, length: usize) {
    let s = generateData(length);
    b.ResetTimer();
    for _ in 0..b.N {
        std::hint::black_box(collator.Key(&s));
    }
}

// immutableKey 对应 Go benchmark helper：ResetTimer 后重复生成不可变 key，避免调用方修改返回值。
fn immutableKey(mut b: testing::B, collator: Box<dyn Collator>, length: usize) {
    let s = generateData(length);
    b.ResetTimer();
    for _ in 0..b.N {
        std::hint::black_box(collator.ImmutableKey(&s));
    }
}

// runCompareBench 对应一组 BenchmarkUtf8mb4*_Compare*，把 Go 的 collator 类型和长度常量组合保留下来。
fn runCompareBench(b: testing::B, _name: &str, collator: Box<dyn Collator>, length: usize) {
    // name 仅用于人工核对 Go benchmark 名称；真实 Go benchmark 由独立函数名承载。
    compare(b, collator, length);
}

// runKeyBench 对应一组 BenchmarkUtf8mb4*_Key*，保留 Key 与 ImmutableKey 两条路径。
fn runKeyBench(
    b: testing::B,
    _name: &str,
    collator: Box<dyn Collator>,
    length: usize,
    immutable: bool,
) {
    if immutable {
        immutableKey(b, collator, length);
    } else {
        key(b, collator, length);
    }
}

// BenchmarkUtf8mb4Bin_CompareShort 到 BenchmarkUtf8mb40900Bin_CompareLong 对应 Go 的 15 个 Compare benchmark。
fn BenchmarkUtf8mb4Bin_CompareShort(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb4Bin_CompareShort",
        Box::new(binPaddingCollator::default()),
        short,
    );
}
fn BenchmarkUtf8mb4GeneralCI_CompareShort(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb4GeneralCI_CompareShort",
        Box::new(generalCICollator::default()),
        short,
    );
}
fn BenchmarkUtf8mb4UnicodeCI_CompareShort(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb4UnicodeCI_CompareShort",
        Box::new(unicodeCICollator::default()),
        short,
    );
}
fn BenchmarkUtf8mb40900AICI_CompareShort(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb40900AICI_CompareShort",
        Box::new(unicode0900AICICollator::default()),
        short,
    );
}
fn BenchmarkUtf8mb40900Bin_CompareShort(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb40900Bin_CompareShort",
        Box::new(derivedBinCollator::default()),
        short,
    );
}
fn BenchmarkUtf8mb4Bin_CompareMid(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb4Bin_CompareMid",
        Box::new(binPaddingCollator::default()),
        middle,
    );
}
fn BenchmarkUtf8mb4GeneralCI_CompareMid(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb4GeneralCI_CompareMid",
        Box::new(generalCICollator::default()),
        middle,
    );
}
fn BenchmarkUtf8mb4UnicodeCI_CompareMid(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb4UnicodeCI_CompareMid",
        Box::new(unicodeCICollator::default()),
        middle,
    );
}
fn BenchmarkUtf8mb40900AICI_CompareMid(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb40900AICI_CompareMid",
        Box::new(unicode0900AICICollator::default()),
        middle,
    );
}
fn BenchmarkUtf8mb40900Bin_CompareMid(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb40900Bin_CompareMid",
        Box::new(derivedBinCollator::default()),
        middle,
    );
}
fn BenchmarkUtf8mb4Bin_CompareLong(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb4Bin_CompareLong",
        Box::new(binPaddingCollator::default()),
        long,
    );
}
fn BenchmarkUtf8mb4GeneralCI_CompareLong(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb4GeneralCI_CompareLong",
        Box::new(generalCICollator::default()),
        long,
    );
}
fn BenchmarkUtf8mb4UnicodeCI_CompareLong(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb4UnicodeCI_CompareLong",
        Box::new(unicodeCICollator::default()),
        long,
    );
}
fn BenchmarkUtf8mb40900AICI_CompareLong(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb40900AICI_CompareLong",
        Box::new(unicode0900AICICollator::default()),
        long,
    );
}
fn BenchmarkUtf8mb40900Bin_CompareLong(b: testing::B) {
    runCompareBench(
        b,
        "BenchmarkUtf8mb40900Bin_CompareLong",
        Box::new(derivedBinCollator::default()),
        long,
    );
}

// Key/ImmutableKey benchmark 保持 Go 的 5 种 collator x 3 种长度 x 2 种 key API 组合。
fn BenchmarkUtf8mb4Bin_KeyShort(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4Bin_KeyShort",
        Box::new(binPaddingCollator::default()),
        short,
        false,
    );
}
fn BenchmarkUtf8mb4Bin_ImmutableKeyShort(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4Bin_ImmutableKeyShort",
        Box::new(binPaddingCollator::default()),
        short,
        true,
    );
}
fn BenchmarkUtf8mb4GeneralCI_KeyShort(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4GeneralCI_KeyShort",
        Box::new(generalCICollator::default()),
        short,
        false,
    );
}
fn BenchmarkUtf8mb4GeneralCI_ImmutableKeyShort(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4GeneralCI_ImmutableKeyShort",
        Box::new(generalCICollator::default()),
        short,
        true,
    );
}
fn BenchmarkUtf8mb4UnicodeCI_KeyShort(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4UnicodeCI_KeyShort",
        Box::new(unicodeCICollator::default()),
        short,
        false,
    );
}
fn BenchmarkUtf8mb4UnicodeCI_ImmutableKeyShort(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4UnicodeCI_ImmutableKeyShort",
        Box::new(unicodeCICollator::default()),
        short,
        true,
    );
}
fn BenchmarkUtf8mb40900AICI_KeyShort(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900AICI_KeyShort",
        Box::new(unicode0900AICICollator::default()),
        short,
        false,
    );
}
fn BenchmarkUtf8mb40900AICI_ImmutableKeyShort(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900AICI_ImmutableKeyShort",
        Box::new(unicode0900AICICollator::default()),
        short,
        true,
    );
}
fn BenchmarkUtf8mb40900Bin_KeyShort(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900Bin_KeyShort",
        Box::new(derivedBinCollator::default()),
        short,
        false,
    );
}
fn BenchmarkUtf8mb40900Bin_ImmutableKeyShort(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900Bin_ImmutableKeyShort",
        Box::new(derivedBinCollator::default()),
        short,
        true,
    );
}
fn BenchmarkUtf8mb4Bin_KeyMid(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4Bin_KeyMid",
        Box::new(binPaddingCollator::default()),
        middle,
        false,
    );
}
fn BenchmarkUtf8mb4Bin_ImmutableKeyMid(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4Bin_ImmutableKeyMid",
        Box::new(binPaddingCollator::default()),
        middle,
        true,
    );
}
fn BenchmarkUtf8mb4GeneralCI_KeyMid(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4GeneralCI_KeyMid",
        Box::new(generalCICollator::default()),
        middle,
        false,
    );
}
fn BenchmarkUtf8mb4GeneralCI_ImmutableKeyMid(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4GeneralCI_ImmutableKeyMid",
        Box::new(generalCICollator::default()),
        middle,
        true,
    );
}
fn BenchmarkUtf8mb4UnicodeCI_KeyMid(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4UnicodeCI_KeyMid",
        Box::new(unicodeCICollator::default()),
        middle,
        false,
    );
}
fn BenchmarkUtf8mb4UnicodeCI_ImmutableKeyMid(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4UnicodeCI_ImmutableKeyMid",
        Box::new(unicodeCICollator::default()),
        middle,
        true,
    );
}
fn BenchmarkUtf8mb40900AICI_KeyMid(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900AICI_KeyMid",
        Box::new(unicode0900AICICollator::default()),
        middle,
        false,
    );
}
fn BenchmarkUtf8mb40900AICI_ImmutableKeyMid(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900AICI_ImmutableKeyMid",
        Box::new(unicode0900AICICollator::default()),
        middle,
        true,
    );
}
fn BenchmarkUtf8mb40900Bin_KeyMid(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900Bin_KeyMid",
        Box::new(derivedBinCollator::default()),
        middle,
        false,
    );
}
fn BenchmarkUtf8mb40900Bin_ImmutableKeyMid(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900Bin_ImmutableKeyMid",
        Box::new(derivedBinCollator::default()),
        middle,
        true,
    );
}
fn BenchmarkUtf8mb4Bin_KeyLong(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4Bin_KeyLong",
        Box::new(binPaddingCollator::default()),
        long,
        false,
    );
}
fn BenchmarkUtf8mb4Bin_ImmutableKeyLong(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4Bin_ImmutableKeyLong",
        Box::new(binPaddingCollator::default()),
        long,
        true,
    );
}
fn BenchmarkUtf8mb4GeneralCI_KeyLong(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4GeneralCI_KeyLong",
        Box::new(generalCICollator::default()),
        long,
        false,
    );
}
fn BenchmarkUtf8mb4GeneralCI_ImmutableKeyLong(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4GeneralCI_ImmutableKeyLong",
        Box::new(generalCICollator::default()),
        long,
        true,
    );
}
fn BenchmarkUtf8mb4UnicodeCI_KeyLong(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4UnicodeCI_KeyLong",
        Box::new(unicodeCICollator::default()),
        long,
        false,
    );
}
fn BenchmarkUtf8mb4UnicodeCI_ImmutableKeyLong(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb4UnicodeCI_ImmutableKeyLong",
        Box::new(unicodeCICollator::default()),
        long,
        true,
    );
}
fn BenchmarkUtf8mb40900AICI_KeyLong(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900AICI_KeyLong",
        Box::new(unicode0900AICICollator::default()),
        long,
        false,
    );
}
fn BenchmarkUtf8mb40900AICI_ImmutableKeyLong(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900AICI_ImmutableKeyLong",
        Box::new(unicode0900AICICollator::default()),
        long,
        true,
    );
}
fn BenchmarkUtf8mb40900Bin_KeyLong(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900Bin_KeyLong",
        Box::new(derivedBinCollator::default()),
        long,
        false,
    );
}
fn BenchmarkUtf8mb40900Bin_ImmutableKeyLong(b: testing::B) {
    runKeyBench(
        b,
        "BenchmarkUtf8mb40900Bin_ImmutableKeyLong",
        Box::new(derivedBinCollator::default()),
        long,
        true,
    );
}

// Go 的 benchmark 不由 `go test` 默认执行；Rust 保留同名入口和完整数据规模，
// 普通单测只冒烟短输入，完整矩阵通过 ignored 测试显式执行。
#[test]
fn benchmark_lengths_match_go() {
    assert_eq!(short, 2 << 4);
    assert_eq!(middle, 2 << 10);
    assert_eq!(long, 2 << 20);
}

#[test]
fn benchmark_short_entry_points_smoke() {
    let _guard = super::collate_test::COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);
    for entry in [
        BenchmarkUtf8mb4Bin_CompareShort,
        BenchmarkUtf8mb4GeneralCI_CompareShort,
        BenchmarkUtf8mb4UnicodeCI_CompareShort,
        BenchmarkUtf8mb40900AICI_CompareShort,
        BenchmarkUtf8mb40900Bin_CompareShort,
    ] {
        entry(testing::B::new(1));
    }
    SetNewCollationEnabledForTest(false);
}

#[test]
#[ignore = "full Go benchmark matrix; run explicitly with --ignored"]
fn benchmark_entry_points_full_matrix() {
    let _guard = super::collate_test::COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);
    let entries: [fn(testing::B); 45] = [
        BenchmarkUtf8mb4Bin_CompareShort,
        BenchmarkUtf8mb4GeneralCI_CompareShort,
        BenchmarkUtf8mb4UnicodeCI_CompareShort,
        BenchmarkUtf8mb40900AICI_CompareShort,
        BenchmarkUtf8mb40900Bin_CompareShort,
        BenchmarkUtf8mb4Bin_CompareMid,
        BenchmarkUtf8mb4GeneralCI_CompareMid,
        BenchmarkUtf8mb4UnicodeCI_CompareMid,
        BenchmarkUtf8mb40900AICI_CompareMid,
        BenchmarkUtf8mb40900Bin_CompareMid,
        BenchmarkUtf8mb4Bin_CompareLong,
        BenchmarkUtf8mb4GeneralCI_CompareLong,
        BenchmarkUtf8mb4UnicodeCI_CompareLong,
        BenchmarkUtf8mb40900AICI_CompareLong,
        BenchmarkUtf8mb40900Bin_CompareLong,
        BenchmarkUtf8mb4Bin_KeyShort,
        BenchmarkUtf8mb4Bin_ImmutableKeyShort,
        BenchmarkUtf8mb4GeneralCI_KeyShort,
        BenchmarkUtf8mb4GeneralCI_ImmutableKeyShort,
        BenchmarkUtf8mb4UnicodeCI_KeyShort,
        BenchmarkUtf8mb4UnicodeCI_ImmutableKeyShort,
        BenchmarkUtf8mb40900AICI_KeyShort,
        BenchmarkUtf8mb40900AICI_ImmutableKeyShort,
        BenchmarkUtf8mb40900Bin_KeyShort,
        BenchmarkUtf8mb40900Bin_ImmutableKeyShort,
        BenchmarkUtf8mb4Bin_KeyMid,
        BenchmarkUtf8mb4Bin_ImmutableKeyMid,
        BenchmarkUtf8mb4GeneralCI_KeyMid,
        BenchmarkUtf8mb4GeneralCI_ImmutableKeyMid,
        BenchmarkUtf8mb4UnicodeCI_KeyMid,
        BenchmarkUtf8mb4UnicodeCI_ImmutableKeyMid,
        BenchmarkUtf8mb40900AICI_KeyMid,
        BenchmarkUtf8mb40900AICI_ImmutableKeyMid,
        BenchmarkUtf8mb40900Bin_KeyMid,
        BenchmarkUtf8mb40900Bin_ImmutableKeyMid,
        BenchmarkUtf8mb4Bin_KeyLong,
        BenchmarkUtf8mb4Bin_ImmutableKeyLong,
        BenchmarkUtf8mb4GeneralCI_KeyLong,
        BenchmarkUtf8mb4GeneralCI_ImmutableKeyLong,
        BenchmarkUtf8mb4UnicodeCI_KeyLong,
        BenchmarkUtf8mb4UnicodeCI_ImmutableKeyLong,
        BenchmarkUtf8mb40900AICI_KeyLong,
        BenchmarkUtf8mb40900AICI_ImmutableKeyLong,
        BenchmarkUtf8mb40900Bin_KeyLong,
        BenchmarkUtf8mb40900Bin_ImmutableKeyLong,
    ];
    for entry in entries {
        entry(testing::B::new(1));
    }
    SetNewCollationEnabledForTest(false);
}
