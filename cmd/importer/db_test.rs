// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

//! `intToDecimalString` 的对照测试。
//!
//! 该文件保持与 Go 版 `cmd/importer/db_test.go` 相同的表驱动样式，专门验证
//! 整数转 decimal 字符串时的文本形态，而不是数值大小本身。
//! 重点覆盖三类容易回归的边界：
//! - 小数位多于整数位时，必须在前面补零。
//! - 小数位为 0 时，结果不能带小数点。
//! - 输入为 0 或较大整数时，仍要按 decimal 精确保留尾部位数。
//! - 断言消息保留输入参数，便于迁移回归时快速定位是哪种切分规则失配。

use crate::db::intToDecimalString;

// 单个用例同时描述原始整数、目标小数位数和期望 SQL 字面量。
// 测试只关心最终字符串，因此把数据组织成静态表，便于与 Go 版本逐项对照。
struct DecimalStringCase {
    int_value: i64,
    decimal: i32,
    expect: &'static str,
}

#[test]
fn test_int_to_decimal_string() {
    // 这些样本覆盖 Go 版已有的补零、截断和“不带小数部分”场景。
    // Rust 迁移必须保持完全相同的输出，否则生成出的 INSERT 文本会发生形态漂移。
    let tests = [
        DecimalStringCase {
            int_value: 100,
            decimal: 3,
            expect: "0.100",
        },
        DecimalStringCase {
            int_value: 100,
            decimal: 1,
            expect: "10.0",
        },
        DecimalStringCase {
            int_value: 100,
            decimal: 0,
            expect: "100",
        },
        DecimalStringCase {
            int_value: 1,
            decimal: 3,
            expect: "0.001",
        },
        DecimalStringCase {
            int_value: 0,
            decimal: 1,
            expect: "0.0",
        },
        DecimalStringCase {
            int_value: 0,
            decimal: 5,
            expect: "0.00000",
        },
        DecimalStringCase {
            int_value: 12,
            decimal: 0,
            expect: "12",
        },
        DecimalStringCase {
            int_value: 999,
            decimal: 1,
            expect: "99.9",
        },
        DecimalStringCase {
            int_value: 1234,
            decimal: 1,
            expect: "123.4",
        },
        DecimalStringCase {
            int_value: 12_345_678,
            decimal: 2,
            expect: "123456.78",
        },
    ];

    // 逐项断言可以在失败时直接指出哪组 `(整数, decimal)` 组合偏离了 Go 语义，
    // 比批量比较更容易定位是补零、切分位置还是零值处理出了问题。
    for test in tests {
        let actual = intToDecimalString(test.int_value, test.decimal);
        assert_eq!(
            actual, test.expect,
            "test failed on ({}, {}): expected {}, but we got {}",
            test.int_value, test.decimal, test.expect, actual
        );
    }
}
