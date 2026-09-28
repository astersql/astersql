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

// `format` 模块单元测试。
//
// 覆盖 Indent/Flat 输出、跨调用缩进状态、Writer 错误传播与短写返回值。

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use super::format::{FlatFormatter, IndentFormatter, OutputFormat};

/// 线程安全的内存 Writer，便于断言 Format 累积输出。
#[derive(Clone, Default)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl SharedWriter {
    /// 将缓冲字节解码为 UTF-8 字符串。
    fn output(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 对照 Go 文档示例：Indent、Flat 与 OutputFormat 转义。
#[test]
fn test_format_matches_go_output() {
    let output = SharedWriter::default();
    let mut formatter = IndentFormatter(output.clone(), "\t");
    let expected = "abc3%e\n\tx\n\ty\nz\n";
    assert_eq!(
        formatter.Format("abc%d%%e%i\nx\ny\n%uz\n", &[&3]).unwrap(),
        expected.len()
    );
    assert_eq!(output.output(), expected);

    let output = SharedWriter::default();
    let mut formatter = FlatFormatter(output.clone());
    // Flat 模式下非零缩进层级的换行被压成空格。
    let expected = "abc3%e x y z\n ";
    assert_eq!(
        formatter
            .Format("abc%d%%e%i\nx\ny\n%uz\n%i\n", &[&3])
            .unwrap(),
        expected.len()
    );
    assert_eq!(output.output(), expected);

    let input = format!("{}{}{}{}{}{}{}", '\'', '\0', "abc", '\n', '\r', '\\', "def");
    assert_eq!(OutputFormat(&input), "''\\0abc\\n\\r\\\\def");
}

/// 校验 `%i`/`%u` 状态跨多次 `Format` 调用保持。
#[test]
fn test_format_keeps_state_across_calls() {
    let output = SharedWriter::default();
    let mut formatter = IndentFormatter(output.clone(), "  ");

    formatter.Format("root%i\nchild\n", &[]).unwrap();
    formatter.Format("sibling\n%udone\n", &[]).unwrap();

    assert_eq!(output.output(), "root\n  child\n  sibling\ndone\n");
}

/// 始终返回错误的 Writer，用于验证错误上抛。
struct ErrorWriter;

impl Write for ErrorWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "controlled failure",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 底层 Writer 失败时应原样返回错误 kind 与消息。
#[test]
fn test_format_propagates_writer_error() {
    let mut formatter = IndentFormatter(ErrorWriter, "\t");
    let error = formatter.Format("value=%d", &[&3]).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(error.to_string(), "controlled failure");
}

/// 限制单次可写字节数，模拟 Go `io.Writer` 短写。
struct ShortWriter {
    limit: usize,
    output: Vec<u8>,
}

impl Write for ShortWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.limit.min(buf.len());
        self.output.extend_from_slice(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 短写无错误时返回已写长度，与 Go fmt.Fprintf 行为一致。
#[test]
fn test_format_preserves_go_short_write_result() {
    let writer = ShortWriter {
        limit: 4,
        output: Vec::new(),
    };
    let mut formatter = IndentFormatter(writer, "\t");

    assert_eq!(formatter.Format("value=%d", &[&3]).unwrap(), 4);
}

/// 整数精度与十六进制前缀后的零填充须与 Go `fmt.Fprintf` 一致。
#[test]
fn test_format_preserves_go_integer_precision_and_prefixed_zero_padding() {
    let output = SharedWriter::default();
    let mut formatter = IndentFormatter(output.clone(), "\t");

    formatter
        .Format("decimal=%.4d hex=%#08x negative=%#08x", &[&7, &255, &-15])
        .unwrap();

    assert_eq!(
        output.output(),
        "decimal=0007 hex=0x0000ff negative=-0x0000f"
    );

    let output = SharedWriter::default();
    let mut formatter = IndentFormatter(output.clone(), "\t");
    formatter
        .Format("zero=%.0d padded=%08.4d", &[&0, &7])
        .unwrap();
    assert_eq!(output.output(), "zero= padded=    0007");
}
