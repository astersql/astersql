// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 运算符元数据与 SQL 恢复路径的迁移对照测试。
//
// 对照 Go 侧用例表，校验每个 Op 的编号、内部名、字面量、关键字标记，
// 以及 Restore 在默认/小写关键字标志下的输出。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use crate::{format, opcode};

#[cfg(test)]
use std::io::{self, Write};

#[cfg(test)]
use format::{DefaultRestoreFlags, NewRestoreCtx, RestoreKeyWordLowercase};
#[cfg(test)]
use opcode::Op;

/// CASES 与 Go 测试表一一对应：(运算符, 编号, String 名, Format 字面量, 是否关键字)。
#[cfg(test)]
const CASES: [(Op, usize, &str, &str, bool); 31] = [
    (Op::LogicAnd, 1, "and", "AND", true),
    (Op::LeftShift, 2, "leftshift", "<<", false),
    (Op::RightShift, 3, "rightshift", ">>", false),
    (Op::LogicOr, 4, "or", "OR", true),
    (Op::GE, 5, "ge", ">=", false),
    (Op::LE, 6, "le", "<=", false),
    (Op::EQ, 7, "eq", "=", false),
    (Op::NE, 8, "ne", "!=", false),
    (Op::LT, 9, "lt", "<", false),
    (Op::GT, 10, "gt", ">", false),
    (Op::Plus, 11, "plus", "+", false),
    (Op::Minus, 12, "minus", "-", false),
    (Op::And, 13, "bitand", "&", false),
    (Op::Or, 14, "bitor", "|", false),
    (Op::Mod, 15, "mod", "%", false),
    (Op::Xor, 16, "bitxor", "^", false),
    (Op::Div, 17, "div", "/", false),
    (Op::Mul, 18, "mul", "*", false),
    (Op::Not, 19, "not", "not ", true),
    (Op::Not2, 20, "!", "!", false),
    (Op::BitNeg, 21, "bitneg", "~", false),
    (Op::IntDiv, 22, "intdiv", "DIV", true),
    (Op::LogicXor, 23, "xor", "XOR", true),
    (Op::NullEQ, 24, "nulleq", "<=>", false),
    (Op::In, 25, "in", "IN", true),
    (Op::Like, 26, "like", "LIKE", true),
    (Op::Case, 27, "case", "CASE", true),
    (Op::Regexp, 28, "regexp", "REGEXP", true),
    (Op::IsNull, 29, "isnull", "IS NULL", true),
    (Op::IsTruth, 30, "istrue", "IS TRUE", true),
    (Op::IsFalsity, 31, "isfalse", "IS FALSE", true),
];

/// 遍历 CASES，确认编号、String、IsKeyword 与 Format 输出与 Go 表一致。
#[test]
fn opcode_metadata_and_format_match_go_table() {
    for (op, number, name, literal, is_keyword) in CASES {
        assert_eq!(op as usize, number);
        assert_eq!(op.String(), name);
        assert_eq!(op.IsKeyword(), is_keyword);

        let mut output = Vec::new();
        op.Format(&mut output);
        assert_eq!(output, literal.as_bytes());
    }
}

/// Go 的 Format 忽略 io.WriteString 返回的错误；Rust 迁移保持相同错误契约。
#[test]
fn format_ignores_writer_errors_like_go() {
    struct AlwaysFails;

    impl Write for AlwaysFails {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("expected test failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    Op::Plus.Format(&mut AlwaysFails);
}

/// 校验 Restore：关键字走 WriteKeyWord（可大小写变换），符号走 WritePlain。
#[test]
fn restore_uses_keyword_and_plain_writer_paths() {
    for (op, _, _, literal, is_keyword) in CASES {
        let mut default_output = Vec::new();
        op.Restore(&mut NewRestoreCtx(DefaultRestoreFlags, &mut default_output))
            .unwrap();
        let default_expected = if is_keyword {
            literal.to_ascii_uppercase()
        } else {
            literal.to_owned()
        };
        assert_eq!(
            default_output,
            default_expected.as_bytes(),
            "default restore for {op:?}"
        );

        let mut lowercase_output = Vec::new();
        op.Restore(&mut NewRestoreCtx(
            RestoreKeyWordLowercase,
            &mut lowercase_output,
        ))
        .unwrap();
        let expected = if is_keyword {
            literal.to_ascii_lowercase()
        } else {
            literal.to_owned()
        };
        assert_eq!(
            lowercase_output,
            expected.as_bytes(),
            "lowercase restore for {op:?}"
        );
    }
}
