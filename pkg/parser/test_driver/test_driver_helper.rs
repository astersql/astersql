// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 解析器测试驱动辅助函数：空白/数字判定、绝对值与十进制位数估算。
//
// 对照 Go `test_driver_helper.go`：仅做内存中的字符判断与整数计算，
// 供 MyDecimal 解析与 DefaultTypeForValue 估算显示宽度（flen）使用。

// 这段逻辑只执行内存中的字符判断和整数计算。
// isSpace 对应 Go 的窄空白判断，只接受空格和制表符，不扩展到全部 Unicode 空白。
/// 窄空白判断：仅空格与制表符（对齐 Go，非全部 Unicode 空白）。
pub fn isSpace(value: u8) -> bool {
    value == b' ' || value == b'\t'
}

// isDigit 对应十进制 ASCII 数字判断。
/// 判断字节是否为 ASCII 十进制数字。
pub fn isDigit(value: u8) -> bool {
    value.is_ascii_digit()
}

// myMin 保持 Go 辅助函数的分支形状。
/// 返回两个 i32 中的较小值。
pub fn myMin(a: i32, b: i32) -> i32 {
    if a < b { a } else { b }
}

// pow10 对应 math.Pow10 后转 int32；超出范围时沿用目标整数转换边界。
/// 计算 10^exponent 并转为 i32（对齐 Go math.Pow10 再截断）。
pub fn pow10(exponent: i32) -> i32 {
    10_f64.powi(exponent) as i32
}

// Abs 使用 Go 原实现的补码位运算，连同 MinInt64 的溢出位模式一并保留。
/// 位运算绝对值；对 i64::MIN 保留与 Go 相同的溢出位模式。
pub fn Abs(value: i64) -> i64 {
    let sign = value >> 63;
    (value ^ sign).wrapping_sub(sign)
}

// uintSizeTable 用阈值比较求十进制长度，比循环除以 10 更直接。
// 下标 0 故意冗余，使 StrLenOfUint64Fast 可以从 1 开始并直接返回下标。
/// 无符号整数十进制位数阈值表；下标即位数，末项为 u64::MAX。
pub static uintSizeTable: [u64; 21] = [
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
    u64::MAX, // 18446744073709551615，共 20 位。
];

// StrLenOfUint64Fast 返回无符号整数的十进制字符数。
/// 返回无符号整数的十进制字符个数（阈值表查找）。
pub fn StrLenOfUint64Fast(value: u64) -> i32 {
    for index in 1..uintSizeTable.len() {
        if value <= uintSizeTable[index] {
            return index as i32;
        }
    }
    // 表尾是 u64::MAX，正常输入不会到达这里；保留显式失败便于发现表结构被破坏。
    unreachable!("uint size table must cover u64::MAX")
}

// StrLenOfInt64Fast 在绝对值位数上为负号额外加一。
/// 返回有符号整数的十进制字符个数（含负号）。
pub fn StrLenOfInt64Fast(value: i64) -> i32 {
    let sign_size = if value < 0 { 1 } else { 0 };
    sign_size + StrLenOfUint64Fast(Abs(value) as u64)
}
