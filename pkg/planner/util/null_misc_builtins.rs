// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and limitations under the License.

// NULL 拒绝证明所用的 builtin 函数属性登记表。
//
// - [`NULL_REJECT_NULL_PRESERVING_FUNCTIONS`]：任一参数为 NULL 则结果为 NULL 的函数；
// - [`NULL_REJECT_REJECT_NULL_TESTS`]：IS TRUE / IS FALSE 等对 NULL 的特殊测试语义。
// 名称字符串与 Go AST 常量对齐，缺省符号以字面量占位。

// 本文件由 pkg/planner/util/null_misc_builtins.go 迁移而来。
// 登记 NULL 拒绝证明使用的 builtin 属性。
// 字符串表保留 ast 常量名称，待 Rust AST 接线时转换为对应枚举或静态符号。

#[derive(Clone, Copy, PartialEq, Eq)]
/// 谓词测试类函数对 NULL 输入的结果模式。
pub enum NullRejectTestMode {
    /// NULL 输入时返回 FALSE（非 NULL）。
    ReturnsFalse,
    /// NULL 输入时结果仍为 NULL。
    KeepsNull,
}

// 与 Go map[string]struct{} 等价的注册表。完整保留原文件中的 ast 名称和声明集合。
use parser_ast::functions as parser_ast;

/// 对 NULL 保持传递的函数名表（任一参数 NULL → 结果 NULL）。
pub static NULL_REJECT_NULL_PRESERVING_FUNCTIONS: &[&str] = &[
    parser_ast::ASCII,
    parser_ast::Abs,
    parser_ast::Acos,
    parser_ast::AddDate,
    parser_ast::AddTime,
    parser_ast::And,
    parser_ast::Asin,
    parser_ast::Atan,
    parser_ast::Atan2,
    parser_ast::Bin,
    parser_ast::BitCount,
    parser_ast::BitLength,
    parser_ast::BitNeg,
    parser_ast::CRC32,
    parser_ast::Cast,
    parser_ast::Ceil,
    parser_ast::Ceiling,
    parser_ast::CharLength,
    parser_ast::CharacterLength,
    parser_ast::Compress,
    parser_ast::Concat,
    parser_ast::Conv,
    parser_ast::ConvertTz,
    parser_ast::Cos,
    parser_ast::Cot,
    "date", // parser_ast::Date is currently missing from the Rust AST surface.
    parser_ast::DateAdd,
    parser_ast::DateDiff,
    parser_ast::DateFormat,
    parser_ast::DateSub,
    "day", // parser_ast::Day is currently missing from the Rust AST surface.
    parser_ast::DayName,
    parser_ast::DayOfMonth,
    parser_ast::DayOfWeek,
    parser_ast::DayOfYear,
    parser_ast::Degrees,
    parser_ast::Div,
    parser_ast::EQ,
    parser_ast::Exp,
    parser_ast::Extract,
    parser_ast::FindInSet,
    parser_ast::Floor,
    parser_ast::FromBase64,
    parser_ast::FromDays,
    parser_ast::FromUnixTime,
    parser_ast::GE,
    parser_ast::GT,
    parser_ast::Greatest,
    parser_ast::Hex,
    "hour", // parser_ast::Hour is currently missing from the Rust AST surface.
    parser_ast::Ilike,
    parser_ast::Inet6Aton,
    parser_ast::Inet6Ntoa,
    parser_ast::InetAton,
    parser_ast::InetNtoa,
    parser_ast::InsertFunc,
    parser_ast::Instr,
    parser_ast::IntDiv,
    parser_ast::JSONContains,
    parser_ast::JSONContainsPath,
    parser_ast::JSONDepth,
    parser_ast::JSONExtract,
    parser_ast::JSONKeys,
    parser_ast::JSONLength,
    parser_ast::JSONMemberOf,
    parser_ast::JSONMerge,
    parser_ast::JSONMergePreserve,
    parser_ast::JSONOverlaps,
    parser_ast::JSONPretty,
    parser_ast::JSONQuote,
    parser_ast::JSONRemove,
    parser_ast::JSONStorageFree,
    parser_ast::JSONStorageSize,
    parser_ast::JSONType,
    parser_ast::JSONUnquote,
    parser_ast::JSONValid,
    parser_ast::LE,
    parser_ast::LT,
    parser_ast::LTrim,
    parser_ast::LastDay,
    parser_ast::Lcase,
    parser_ast::Least,
    parser_ast::Left,
    parser_ast::LeftShift,
    parser_ast::Length,
    parser_ast::Like,
    parser_ast::Ln,
    parser_ast::Locate,
    parser_ast::Log,
    parser_ast::Log10,
    parser_ast::Log2,
    parser_ast::LogicXor,
    parser_ast::Lower,
    parser_ast::Lpad,
    parser_ast::MD5,
    parser_ast::MakeDate,
    parser_ast::MakeTime,
    parser_ast::MicroSecond,
    parser_ast::Mid,
    parser_ast::Minus,
    "minute", // parser_ast::Minute is currently missing from the Rust AST surface.
    parser_ast::Mod,
    "month", // parser_ast::Month is currently missing from the Rust AST surface.
    parser_ast::MonthName,
    parser_ast::Mul,
    parser_ast::NE,
    parser_ast::Oct,
    parser_ast::OctetLength,
    parser_ast::Or,
    parser_ast::Ord,
    parser_ast::PeriodAdd,
    parser_ast::PeriodDiff,
    parser_ast::Plus,
    parser_ast::Position,
    parser_ast::Pow,
    parser_ast::Power,
    "quarter", // parser_ast::Quarter is currently missing from the Rust AST surface.
    parser_ast::RTrim,
    parser_ast::Radians,
    parser_ast::Regexp,
    parser_ast::RegexpInStr,
    parser_ast::RegexpLike,
    parser_ast::RegexpReplace,
    parser_ast::RegexpSubstr,
    parser_ast::Repeat,
    parser_ast::Replace,
    parser_ast::Reverse,
    parser_ast::Right,
    parser_ast::RightShift,
    parser_ast::Round,
    parser_ast::Rpad,
    parser_ast::SHA,
    parser_ast::SHA1,
    parser_ast::SHA2,
    parser_ast::SM3,
    parser_ast::SecToTime,
    "second", // parser_ast::Second is currently missing from the Rust AST surface.
    parser_ast::Sign,
    parser_ast::Sin,
    parser_ast::Space,
    parser_ast::Sqrt,
    parser_ast::StrToDate,
    parser_ast::Strcmp,
    parser_ast::SubDate,
    parser_ast::SubTime,
    parser_ast::Substr,
    parser_ast::Substring,
    parser_ast::SubstringIndex,
    parser_ast::Tan,
    "time", // parser_ast::Time is currently missing from the Rust AST surface.
    parser_ast::TimeDiff,
    parser_ast::TimeFormat,
    parser_ast::TimeToSec,
    parser_ast::Timestamp,
    parser_ast::TimestampAdd,
    parser_ast::TimestampDiff,
    parser_ast::ToBase64,
    parser_ast::ToDays,
    parser_ast::ToSeconds,
    parser_ast::Translate,
    parser_ast::Trim,
    parser_ast::Ucase,
    parser_ast::UnaryMinus,
    parser_ast::UnaryNot,
    parser_ast::Uncompress,
    parser_ast::UncompressedLength,
    parser_ast::Unhex,
    parser_ast::UnixTimestamp,
    parser_ast::Upper,
    parser_ast::WeekOfYear,
    parser_ast::Weekday,
    parser_ast::WeightString,
    parser_ast::Xor,
    "year", // parser_ast::Year is currently missing from the Rust AST surface.
];

/// IS TRUE / IS FALSE 等测试函数及其 NULL 语义模式。
pub static NULL_REJECT_REJECT_NULL_TESTS: &[(&str, NullRejectTestMode)] = &[
    (
        parser_ast::IsTruthWithoutNull,
        NullRejectTestMode::ReturnsFalse,
    ),
    (parser_ast::IsTruthWithNull, NullRejectTestMode::KeepsNull),
    (parser_ast::IsFalsity, NullRejectTestMode::ReturnsFalse),
];

/// 函数名是否登记为 NULL 传递。
pub fn is_null_reject_null_preserving(name: &str) -> bool {
    NULL_REJECT_NULL_PRESERVING_FUNCTIONS
        .iter()
        .any(|candidate| *candidate == name)
}

/// 查询测试类函数的 NULL 结果模式；未登记则返回 None。
pub fn null_reject_test_mode(name: &str) -> Option<NullRejectTestMode> {
    NULL_REJECT_REJECT_NULL_TESTS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, mode)| *mode)
}
