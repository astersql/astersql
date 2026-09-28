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
// MyDecimal 基准风格测试：舍入、转 float、二进制编码与 hash key 的热路径调用。
//
// 以确定性用例替代 Go 随机夹具，避免 flaky；本身用 `#[test]` 驱动循环以覆盖性能关键路径。

#![allow(non_snake_case)]

use types_decimal::mydecimal::*;

/// 生成十进制夹具数量，对齐 Go 基准中的批量规模。
const NUM_TEST_DEC: usize = 1000;

/// 从字符串解析 MyDecimal，解析失败则 panic。
fn decimal(input: &str) -> MyDecimal {
    let mut value = MyDecimal::default();
    value.FromString(input.as_bytes()).unwrap();
    value
}

/// ToBin / ToHashKey 共用的代表性十进制字符串集合。
fn benchmarkMyDecimalToBinOrHashCases() -> [&'static str; 12] {
    [
        "1.000000000000",
        "3",
        "12.000000000",
        "120",
        "120000",
        "100000000000.00000",
        "0.000000001200000000",
        "98765.4321",
        "-123.456000000000000000",
        "0",
        "0000000000",
        "0.00000000000",
    ]
}

#[test]
/// 对多种 scale 与三种舍入模式（HalfUp/Truncate/Ceiling）执行 Round。
fn BenchmarkRound() {
    let cases = [
        ("123456789.987654321", 1),
        ("15.1", 0),
        ("15.5", 0),
        ("15.9", 0),
        ("-15.1", 0),
        ("-15.5", 0),
        ("-15.9", 0),
        ("15.1", 1),
        ("-15.1", 1),
        ("15.17", 1),
        ("15.4", -1),
        ("-15.4", -1),
        ("5.4", -1),
        (".999", 0),
        ("999999999", -9),
    ];
    for (input, scale) in cases {
        let input = decimal(input);
        // 三种舍入模式各跑一遍
        for mode in [ModeHalfUp, ModeTruncate, ModeCeiling] {
            let mut output = MyDecimal::default();
            input.Round(&mut output, scale, mode).unwrap();
        }
    }
}

/// 确定性生成 NUM_TEST_DEC 个不同精度/小数位的 MyDecimal。
fn generated_decimals() -> Vec<MyDecimal> {
    (0..NUM_TEST_DEC)
        .map(|index| {
            // Deterministic counterpart of Go's random fixture: retain varied
            // precision and scale without making the test flaky.
            let digits = (index % 12 + 1) as i32;
            let offset = (index % digits as usize + 1) as i32;
            let base = ((index * 7919 % 1_000_000) as f64) / 1_000_000.0;
            let value = (base * 10_f64.powi(digits)).round() / 10_f64.powi(digits - offset);
            NewDecFromFloatForTest(value)
        })
        .collect()
}

#[test]
/// 走 ToFloat64 原生路径。
fn BenchmarkToFloat64New() {
    for value in generated_decimals() {
        value.ToFloat64().unwrap();
    }
}

#[test]
/// 经 String 再 parse 的旧路径对照。
fn BenchmarkToFloat64Old() {
    for value in generated_decimals() {
        value.String().parse::<f64>().unwrap();
    }
}

#[test]
/// 按自身 PrecisionAndFrac 编码为二进制十进制。
fn BenchmarkMyDecimalToBin() {
    for input in benchmarkMyDecimalToBinOrHashCases() {
        let value = decimal(input);
        let (precision, frac) = value.PrecisionAndFrac();
        value.ToBin(precision as isize, frac as isize).1.unwrap();
    }
}

#[test]
/// 计算可用于比较/索引的 hash key 字节。
fn BenchmarkMyDecimalToHashKey() {
    for input in benchmarkMyDecimalToBinOrHashCases() {
        decimal(input).ToHashKey().unwrap();
    }
}
