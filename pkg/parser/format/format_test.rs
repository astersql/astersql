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

// `format` 模块的单元测试：覆盖缩进/扁平格式化、`RestoreCtx` 标志优先级，
// 以及 TiDB 特殊注释的成功与错误传播路径。
//
// 与 Go 侧 `format_test.go` 中的用例语义对齐。

use crate::*;
use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;
use std::sync::Arc;

/// 一组恢复标志与期望输出字符串的对照用例。
struct RestoreCase {
    /// 本用例使用的恢复标志。
    flag: RestoreFlags,
    /// 关键字 + 字符串 + 标识符依次写入后的期望全文。
    expect: &'static str,
}

/// 可在格式化器与断言之间共享的内存缓冲区。
#[derive(Clone, Default)]
struct SharedBuffer(Rc<RefCell<Vec<u8>>>);

impl SharedBuffer {
    /// 将缓冲内容解码为 UTF-8 字符串。
    fn string(&self) -> String {
        String::from_utf8(self.0.borrow().clone()).unwrap()
    }
}

impl Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 每次最多写入指定字节数，用于模拟 Go `io.Writer` 的无错误短写。
struct ShortWriter {
    output: SharedBuffer,
    limit: usize,
}

impl Write for ShortWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.limit.min(bytes.len());
        self.output.write(&bytes[..written])
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 调用 `Format` 后比较缓冲区与期望值；固定传入参数 `3` 以覆盖 `%d`。
// check_format 对应 Go 的 checkFormat：执行格式化后读取完整缓冲区并比较。
fn check_format<F: Formatter>(mut formatter: F, output: &SharedBuffer, input: &str, expect: &str) {
    let three = 3;
    formatter.Format(input, &[&three]).unwrap();
    drop(formatter);
    assert_eq!(output.string(), expect);
}

/// 验证 `IndentFormatter` 与 `FlatFormatter` 对 `%i`/`%u`/`%%`/`%d` 的处理。
// 对应 Go TestFormat。
#[test]
fn test_format() {
    let output = SharedBuffer::default();
    check_format(
        IndentFormatter(output.clone(), "\t"),
        &output,
        "abc%d%%e%i\nx\ny\n%uz\n",
        "abc3%e\n\tx\n\ty\nz\n",
    );

    let output = SharedBuffer::default();
    check_format(
        FlatFormatter(output.clone()),
        &output,
        "abc%d%%e%i\nx\ny\n%uz\n%i\n",
        "abc3%e x y z\n ",
    );
}

/// Go `fmt.Fprintf` 对无错误短写只返回本次实际写入长度，不会补写剩余内容。
#[test]
fn test_format_preserves_short_write_result() {
    let output = SharedBuffer::default();
    let mut formatter = IndentFormatter(
        ShortWriter {
            output: output.clone(),
            limit: 4,
        },
        "\t",
    );

    assert_eq!(formatter.Format("value=%d", &[&3]).unwrap(), 4);
    assert_eq!(output.string(), "valu");
}

/// 验证 `WriteKeyWord`/`WriteString`/`WriteName` 在各标志及冲突组合下的输出与优先级。
// 对应 Go TestRestoreCtx，完整保留 15 组标志及冲突标志的优先级。
#[test]
fn test_restore_ctx() {
    let test_cases = [
        RestoreCase {
            flag: RestoreFlags(0),
            expect: "key`.'\"Word\\ str`.'\"ing\\ na`.'\"Me\\",
        },
        RestoreCase {
            flag: RestoreStringSingleQuotes,
            expect: "key`.'\"Word\\ 'str`.''\"ing\\' na`.'\"Me\\",
        },
        RestoreCase {
            flag: RestoreStringDoubleQuotes,
            expect: "key`.'\"Word\\ \"str`.'\"\"ing\\\" na`.'\"Me\\",
        },
        RestoreCase {
            flag: RestoreStringEscapeBackslash,
            expect: "key`.'\"Word\\ str`.'\"ing\\\\ na`.'\"Me\\",
        },
        RestoreCase {
            flag: RestoreKeyWordUppercase,
            expect: "KEY`.'\"WORD\\ str`.'\"ing\\ na`.'\"Me\\",
        },
        RestoreCase {
            flag: RestoreKeyWordLowercase,
            expect: "key`.'\"word\\ str`.'\"ing\\ na`.'\"Me\\",
        },
        RestoreCase {
            flag: RestoreNameUppercase,
            expect: "key`.'\"Word\\ str`.'\"ing\\ NA`.'\"ME\\",
        },
        RestoreCase {
            flag: RestoreNameLowercase,
            expect: "key`.'\"Word\\ str`.'\"ing\\ na`.'\"me\\",
        },
        RestoreCase {
            flag: RestoreNameDoubleQuotes,
            expect: "key`.'\"Word\\ str`.'\"ing\\ \"na`.'\"\"Me\\\"",
        },
        RestoreCase {
            flag: RestoreNameBackQuotes,
            expect: "key`.'\"Word\\ str`.'\"ing\\ `na``.'\"Me\\`",
        },
        RestoreCase {
            flag: DefaultRestoreFlags,
            expect: "KEY`.'\"WORD\\ 'str`.''\"ing\\' `na``.'\"Me\\`",
        },
        RestoreCase {
            flag: RestoreStringSingleQuotes | RestoreStringDoubleQuotes,
            expect: "key`.'\"Word\\ 'str`.''\"ing\\' na`.'\"Me\\",
        },
        RestoreCase {
            flag: RestoreKeyWordUppercase | RestoreKeyWordLowercase,
            expect: "KEY`.'\"WORD\\ str`.'\"ing\\ na`.'\"Me\\",
        },
        RestoreCase {
            flag: RestoreNameUppercase | RestoreNameLowercase,
            expect: "key`.'\"Word\\ str`.'\"ing\\ NA`.'\"ME\\",
        },
        RestoreCase {
            flag: RestoreNameDoubleQuotes | RestoreNameBackQuotes,
            expect: "key`.'\"Word\\ str`.'\"ing\\ \"na`.'\"\"Me\\\"",
        },
    ];

    for test_case in test_cases {
        let mut output = Vec::new();
        let mut ctx = NewRestoreCtx(test_case.flag, &mut output);
        ctx.WriteKeyWord("key`.'\"Word\\").unwrap();
        ctx.WritePlain(" ").unwrap();
        ctx.WriteString("str`.'\"ing\\").unwrap();
        ctx.WritePlain(" ").unwrap();
        ctx.WriteName("na`.'\"Me\\").unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            test_case.expect,
            "flags={}",
            test_case.flag.0
        );
    }
}

/// 可放入 `io::Error` 的哨兵错误类型，用于指针相等断言。
#[derive(Debug)]
struct ErrorIdentity;

impl std::fmt::Display for ErrorIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("xxxx")
    }
}

impl std::error::Error for ErrorIdentity {}

/// 验证特殊注释包装：带 feature id、空 id，以及回调错误原样返回且不补写结尾。
// 对应 Go TestRestoreSpecialComment，覆盖 feature id、空 id 和原错误返回。
#[test]
fn test_restore_special_comment() {
    let mut output = Vec::new();
    {
        let mut ctx = NewRestoreCtx(RestoreTiDBSpecialComment, &mut output);
        ctx.WriteWithSpecialComments("fea_id", |ctx| ctx.WritePlain("content"))
            .unwrap();
    }
    assert_eq!(output, b"/*T![fea_id] content */");

    output.clear();
    {
        let mut ctx = NewRestoreCtx(RestoreTiDBSpecialComment, &mut output);
        ctx.WriteWithSpecialComments("", |ctx| ctx.WritePlain("shard_row_id_bits"))
            .unwrap();
    }
    assert_eq!(output, b"/*T! shard_row_id_bits */");

    output.clear();
    let identity = Arc::new(ErrorIdentity);
    let error = io::Error::other(identity.clone());
    let mut ctx = NewRestoreCtx(RestoreTiDBSpecialComment, &mut output);
    let got = ctx
        .WriteWithSpecialComments("", |_| Err(error))
        .unwrap_err();
    let returned_identity = got
        .get_ref()
        .and_then(|error| error.downcast_ref::<Arc<ErrorIdentity>>())
        .unwrap();
    assert!(Arc::ptr_eq(&identity, returned_identity));
}
