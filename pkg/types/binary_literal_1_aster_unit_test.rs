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

// 迁移期单元测试：二进制字面量、类型 Context 标志与数值转换对齐 Go。
//
// 覆盖 BIT/HEX 解析与格式化、Flags/Context 不可变更新，
// 以及向量比较、科学计数法与截断转换语义。

use super::*;

/// 校验去前导零、BIT/HEX 解析、位串格式化及溢出截断为 u64::MAX。
#[test]
fn binary_literal_parsing_formatting_and_integer_conversion_match_go() {
    assert_eq!(trimLeadingZeroBytes(&[0, 0, 1, 0]), &[1, 0]);
    assert_eq!(NewBinaryLiteralFromUint(0x123, -1).as_ref(), &[1, 0x23]);
    assert_eq!(NewBinaryLiteralFromUint(0x123, 1).as_ref(), &[0x23]);

    // BIT 字面量：按位解析后可再格式化为紧凑 b'...' 形式
    let bit = ParseBitStr("b'000000010'".to_owned()).unwrap();
    assert_eq!(bit.as_ref(), &[0, 2]);
    assert_eq!(bit.ToBitLiteralString(true), "b'10'");
    assert!(ParseBitStr("0b123".to_owned()).is_err());

    // HEX 字面量：字节内容与 0x 字符串表示
    let hex = ParseHexStr("0x4D7953514C".to_owned()).unwrap();
    assert_eq!(hex.ToString(), "MySQL");
    assert_eq!(hex.String(), "0x4d7953514c");
    assert!(ParseHexStr("x'1'".to_owned()).is_err());

    // 超过 8 字节时 ToInt 在严格模式下截断并返回错误
    let overflow = ParseHexStr("0x1010ffff8080ff12ff".to_owned())
        .unwrap()
        .ToInt(StrictContext.clone())
        .unwrap_err();
    assert_eq!(overflow.value, u64::MAX);
    assert!(overflow.to_string().contains("truncated"));
}

/// 校验 Flags 读写与 WithFlags 不修改原 Context（不可变更新）。
#[test]
fn context_flags_are_immutable_and_warning_policy_matches_go() {
    let flags = StrictFlags
        .WithAllowNegativeToUnsigned(true)
        .WithSkipSACIICheck(true)
        .WithSkipUTF8Check(true)
        .WithSkipUTF8MB4Check(true);
    assert!(flags.AllowNegativeToUnsigned());
    assert!(flags.SkipASCIICheck());
    assert!(flags.SkipUTF8Check());
    assert!(flags.SkipUTF8MB4Check());
    assert_eq!(StrictFlags, Flags(0));

    // 原 Context 保持 StrictFlags，新实例携带更新后的 flags
    let original = StrictContext.clone();
    let changed = original.WithFlags(flags);
    assert_eq!(original.Flags(), StrictFlags);
    assert_eq!(changed.Flags(), flags);
    assert_eq!(changed.Location(), chrono_tz::UTC);
}

/// 校验有符号/无符号向量比较、标量 CompareInt 与校对规则比较。
#[test]
fn vector_scalar_and_collation_comparisons_match_go() {
    let mut result = [0; 4];
    // 无符号对有符号：负数或越界无符号视为更大
    VecCompareUI(
        &[0, 1, i64::MAX as u64 + 1, 9],
        &[0, -1, 0, 10],
        &mut result,
    );
    assert_eq!(result, [0, 1, 1, -1]);

    VecCompareIU(
        &[-1, 1, i64::MAX, 9],
        &[0, 1, i64::MAX as u64 + 1, 8],
        &mut result,
    );
    assert_eq!(result, [-1, 0, -1, 1]);

    assert_eq!(CompareInt(-1, false, 1, true), -1);
    assert_eq!(CompareInt(-1, true, 1, false), 1);
    assert_eq!(CompareInt(-1, true, -1, true), 0);
    // utf8mb4_general_ci：大小写不敏感且忽略尾随空格
    assert_eq!(CompareString("A ", "a", "utf8mb4_general_ci"), 0);
}

/// 校验科学计数法展开、四舍五入、范围裁剪与字符串截断转换。
#[test]
fn numeric_and_string_conversions_keep_go_rounding_and_clipped_values() {
    assert_eq!(convertScientificNotation("1E6").unwrap(), "1000000");
    assert_eq!(convertScientificNotation(".12345E+5").unwrap(), "12345");
    assert_eq!(
        convertScientificNotation("123.456E-5").unwrap(),
        "0.00123456"
    );

    assert_eq!(roundIntStr(b'5', "9"), "10");
    assert_eq!(roundIntStr(b'5', "-9"), "-10");
    assert_eq!(floatStrToIntStr("1.5", "1.5").0, "2");
    assert_eq!(floatStrToIntStr("-1.5", "-1.5").0, "-2");

    assert_eq!(
        ConvertFloatToInt(1.5, i8::MIN as i64, i8::MAX as i64, mysql::TypeTiny).unwrap(),
        2
    );
    // 越界时裁剪到类型上下界并附带溢出错误
    let clipped =
        ConvertIntToInt(256, i8::MIN as i64, i8::MAX as i64, mysql::TypeTiny).unwrap_err();
    assert_eq!(clipped.value, i8::MAX as i64);

    // IgnoreTruncateErr：解析前缀数字并忽略尾部非法字符
    let ignored = DefaultStmtNoWarningContext
        .clone()
        .WithFlags(DefaultStmtFlags.WithIgnoreTruncateErr(true));
    assert_eq!(StrToInt(ignored, "12xyz", true).unwrap(), 12);
    assert_eq!(StrToUint(StrictContext.clone(), "-000", true).unwrap(), 0);
    assert_eq!(truncateStr("hello".to_owned(), 3), "hel");
}
