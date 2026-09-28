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

// Parquet DECIMAL 基准场景的正确性回归测试。
//
// 这些精简样例覆盖大端二补码的正负整数，并确认 `scale` 会从整数末尾
// 划分出对应位数的小数部分，防止性能实现调整破坏十进制文本结果。

use crate::type_converter::{decimal_bytes_to_string, getStringFromParquetByte};

fn decimal_bench_cases() -> Vec<(&'static str, Vec<u8>, i32, &'static str)> {
    let pattern = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
    let make_full = |len: usize| pattern.into_iter().cycle().take(len).collect();

    vec![
        ("SmallPos", vec![0x01], 3, "0.001"),
        ("SmallNeg", vec![0xff], 4, "-0.0001"),
        ("Medium", make_full(8), 6, "81985529216.486895"),
        (
            "Long",
            make_full(16),
            8,
            "15123660752041709290495823544.06559215",
        ),
        ("LongNeg", vec![0xff; 16], 12, "-0.000000000001"),
    ]
}

#[test]
fn decimal_benchmark_cases_parse_positive_and_negative_twos_complement() {
    for (name, raw, scale, expected) in decimal_bench_cases() {
        let original = raw.clone();
        assert_eq!(
            decimal_bytes_to_string(&raw, scale).unwrap(),
            expected,
            "case {name}"
        );
        assert_eq!(
            getStringFromParquetByte(&raw, scale).unwrap(),
            expected,
            "case {name} through the Go-named compatibility entry point"
        );
        assert_eq!(raw, original, "case {name} must be reusable across runs");
    }
}
