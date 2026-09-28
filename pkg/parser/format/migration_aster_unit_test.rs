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

// 迁移补充测试：在独立编译路径下直接引入 `format.rs`，
// 验证跨调用状态机、UTF-8/宽度、`OutputFormat`、恢复标志优先级、
// 特殊注释部分写出与 CTE 作用域截断等行为。

#[path = "format.rs"]
mod format;

use format::*;
use std::fmt;
use std::io::{self, Write};

/// 按 `flat` 选择扁平或缩进格式化器，连续执行多次 `Format` 后返回全部字节。
fn format_into_bytes(flat: bool, calls: &[(&str, &[&dyn fmt::Display])]) -> Vec<u8> {
    let mut output = Vec::new();
    if flat {
        let mut formatter = FlatFormatter(&mut output);
        for (template, args) in calls {
            formatter.Format(template, args).unwrap();
        }
    } else {
        let mut formatter = IndentFormatter(&mut output, "\t");
        for (template, args) in calls {
            formatter.Format(template, args).unwrap();
        }
    }
    output
}

/// 对照 Go：单次缩进/扁平输出，以及跨两次 `Format` 调用保留的缩进状态。
#[test]
fn formatter_matches_go_indent_flat_and_cross_call_state() {
    let three = 3;
    let args: &[&dyn fmt::Display] = &[&three];
    assert_eq!(
        format_into_bytes(false, &[("abc%d%%e%i\nx\ny\n%uz\n", args)]),
        b"abc3%e\n\tx\n\ty\nz\n"
    );
    assert_eq!(
        format_into_bytes(true, &[("abc%d%%e%i\nx\ny\n%uz\n%i\n", args)]),
        b"abc3%e x y z\n "
    );
    assert_eq!(
        format_into_bytes(false, &[("%i\nfirst\n", &[]), ("second\n%uend", &[])]),
        b"\n\tfirst\n\tsecond\nend"
    );
}

/// 验证多字节 UTF-8 字符与 `%03d` 零填充宽度仍正确。
#[test]
fn formatter_preserves_utf8_and_printf_width() {
    let seven = 7;
    let args: &[&dyn fmt::Display] = &[&seven];
    assert_eq!(
        format_into_bytes(false, &[("表=%03d\n%i列\n%u", args)]),
        "表=007\n\t列\n".as_bytes()
    );
}

/// 验证 `OutputFormat` 对 NUL、引号、换行、回车的转义与中文保留。
#[test]
fn output_format_matches_go_replacements() {
    assert_eq!(OutputFormat("a\0'b\n\r\\中"), "a\\0''b\\n\\r\\中");
}

/// 与 Go 一致的 15 组恢复标志及冲突优先级对照。
#[test]
fn restore_ctx_matches_go_flags_and_priority() {
    let cases = [
        (RestoreFlags(0), "key`.'\"Word\\ str`.'\"ing\\ na`.'\"Me\\"),
        (
            RestoreStringSingleQuotes,
            "key`.'\"Word\\ 'str`.''\"ing\\' na`.'\"Me\\",
        ),
        (
            RestoreStringDoubleQuotes,
            "key`.'\"Word\\ \"str`.'\"\"ing\\\" na`.'\"Me\\",
        ),
        (
            RestoreStringEscapeBackslash,
            "key`.'\"Word\\ str`.'\"ing\\\\ na`.'\"Me\\",
        ),
        (
            RestoreKeyWordUppercase,
            "KEY`.'\"WORD\\ str`.'\"ing\\ na`.'\"Me\\",
        ),
        (
            RestoreKeyWordLowercase,
            "key`.'\"word\\ str`.'\"ing\\ na`.'\"Me\\",
        ),
        (
            RestoreNameUppercase,
            "key`.'\"Word\\ str`.'\"ing\\ NA`.'\"ME\\",
        ),
        (
            RestoreNameLowercase,
            "key`.'\"Word\\ str`.'\"ing\\ na`.'\"me\\",
        ),
        (
            RestoreNameDoubleQuotes,
            "key`.'\"Word\\ str`.'\"ing\\ \"na`.'\"\"Me\\\"",
        ),
        (
            RestoreNameBackQuotes,
            "key`.'\"Word\\ str`.'\"ing\\ `na``.'\"Me\\`",
        ),
        (
            DefaultRestoreFlags,
            "KEY`.'\"WORD\\ 'str`.''\"ing\\' `na``.'\"Me\\`",
        ),
        (
            RestoreStringSingleQuotes | RestoreStringDoubleQuotes,
            "key`.'\"Word\\ 'str`.''\"ing\\' na`.'\"Me\\",
        ),
        (
            RestoreKeyWordUppercase | RestoreKeyWordLowercase,
            "KEY`.'\"WORD\\ str`.'\"ing\\ na`.'\"Me\\",
        ),
        (
            RestoreNameUppercase | RestoreNameLowercase,
            "key`.'\"Word\\ str`.'\"ing\\ NA`.'\"ME\\",
        ),
        (
            RestoreNameDoubleQuotes | RestoreNameBackQuotes,
            "key`.'\"Word\\ str`.'\"ing\\ \"na`.'\"\"Me\\\"",
        ),
    ];

    for (flags, expected) in cases {
        let mut output = Vec::new();
        let mut ctx = NewRestoreCtx(flags, &mut output);
        ctx.WriteKeyWord("key`.'\"Word\\").unwrap();
        ctx.WritePlain(" ").unwrap();
        ctx.WriteString("str`.'\"ing\\").unwrap();
        ctx.WritePlain(" ").unwrap();
        ctx.WriteName("na`.'\"Me\\").unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            expected,
            "flags={}",
            flags.0
        );
    }
}

/// 写入若干次后失败的测试用 `Write`，用于观察特殊注释的部分输出。
#[derive(Default)]
struct FailAfterPrefix {
    /// 已成功写入的字节。
    bytes: Vec<u8>,
    /// 剩余允许成功写入的次数，耗尽后返回错误。
    writes_left: usize,
}

impl Write for FailAfterPrefix {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.writes_left == 0 {
            return Err(io::Error::new(io::ErrorKind::Other, "injected"));
        }
        self.writes_left -= 1;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 成功路径写出完整特殊注释；写入中途失败时保留已写出的 `/*T!` 前缀。
#[test]
fn special_comments_propagate_errors_and_keep_go_partial_output() {
    let mut output = Vec::new();
    {
        let mut ctx = NewRestoreCtx(RestoreTiDBSpecialComment, &mut output);
        ctx.WriteWithSpecialComments("fea_id", |ctx| ctx.WritePlain("content"))
            .unwrap();
    }
    assert_eq!(output, b"/*T![fea_id] content */");

    let mut writer = FailAfterPrefix {
        bytes: Vec::new(),
        writes_left: 1,
    };
    let mut ctx = NewRestoreCtx(RestoreTiDBSpecialComment, &mut writer);
    let error = ctx
        .WriteWithSpecialComments("", |ctx| ctx.WritePlain("content"))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert_eq!(writer.bytes, b"/*T!");
}

/// `RestoreCTEFunc` 退出时只截断本作用域新增名称，并保持大小写敏感匹配。
#[test]
fn cte_scope_restore_truncates_only_new_names() {
    let mut restorer = CTERestorer::default();
    restorer.RecordCTEName("outer");
    let restore = restorer.RestoreCTEFunc();
    restorer.RecordCTEName("inner");
    assert!(restorer.IsCTETableName("outer"));
    assert!(restorer.IsCTETableName("inner"));
    assert!(!restorer.IsCTETableName("INNER"));
    restore(&mut restorer);
    assert_eq!(restorer.CTENames, ["outer"]);
}
