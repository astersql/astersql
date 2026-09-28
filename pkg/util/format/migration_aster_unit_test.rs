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

// format 迁移回归单测。
//
// 对照 Go 输出校验 Indent/Flat/转义，以及 printf 标志、宽度与精度。

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use super::{FlatFormatter, IndentFormatter, OutputFormat};

/// 线程安全内存 Writer，用于断言累积文本。
#[derive(Clone, Default)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl SharedWriter {
    /// 解码缓冲为 UTF-8 字符串。
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

/// 迁移对照：Indent、Flat 与 OutputFormat 的 Go 文档示例输出。
#[test]
fn migration_matches_go_indent_flatten_and_escape_behavior() {
    let output = SharedWriter::default();
    let mut formatter = IndentFormatter(output.clone(), "\t");
    formatter.Format("abc%d%%e%i\nx\ny\n%uz\n", &[&3]).unwrap();
    assert_eq!(output.output(), "abc3%e\n\tx\n\ty\nz\n");

    let output = SharedWriter::default();
    let mut formatter = FlatFormatter(output.clone());
    formatter
        .Format("abc%d%%e%i\nx\ny\n%uz\n%i\n", &[&3])
        .unwrap();
    assert_eq!(output.output(), "abc3%e x y z\n ");

    assert_eq!(OutputFormat("'\0abc\n\r\\def"), "''\\0abc\\n\\r\\\\def");
}

/// 迁移对照：零填充、十六进制、浮点精度与左对齐字符串宽度。
#[test]
fn migration_preserves_go_printf_flags_width_and_precision() {
    let output = SharedWriter::default();
    let mut formatter = IndentFormatter(output.clone(), "  ");

    formatter
        .Format(
            "value=%04d hex=%x ratio=%.2f text=%-5s",
            &[&7, &255, &3.14159, &"go"],
        )
        .unwrap();

    assert_eq!(output.output(), "value=0007 hex=ff ratio=3.14 text=go   ");
}
