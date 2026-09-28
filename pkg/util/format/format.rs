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

// Copyright (c) 2014 The sortutil Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/STRUTIL-LICENSE file.

// 本文件由 pkg/util/format/format.go 迁移而来，保留 Go 实现结构。

// 缩进/扁平文本格式化工具。
//
// 对应 Go `pkg/util/format`：提供带 `%i`/`%u` 缩进命令的 `IndentFormatter`、
// 嵌套换行压平的 `FlatFormatter`，以及 SQL 风格字符转义的 `OutputFormat`。
// Format 状态机按字节扫描格式串，再模拟 Go `fmt.Fprintf` 的 `%d/%s/%v` 等写入。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::fmt;
use std::io::{self, Write};

// Go const iota 迁移：这些状态驱动 format 方法的逐字节扫描状态机。
/// 普通正文状态（非行首）。
const st0: i32 = 0;
/// 行首（Beginning Of Line），待输出缩进。
const stBOL: i32 = 1;
/// 刚读到 `%`，等待判定 `%i`/`%u` 或普通百分号片段。
const stPERC: i32 = 2;
/// 行首刚读到 `%`，缩进命令不触发行首缩进写出。
const stBOLPERC: i32 = 3;

// Formatter is an io.Writer extended formatter by a fmt.Printf like function Format.
// Formatter 对应 Go 接口：它既是 Writer，又额外暴露类似 fmt.Printf 的 Format 方法。
// Rust 用 Display trait object 表达当前调用方需要的 Go ...any 格式参数。
/// Format 参数：用 `Display` 近似 Go 的 `...any`。
pub type FormatArg<'a> = &'a dyn fmt::Display;

/// 扩展 Writer：在 write 之外提供类似 `fmt.Printf` 的 `Format`。
pub trait Formatter: Write {
    /// 按格式串与参数写出，返回底层 Writer 的写入字节数。
    fn Format(&mut self, format: &str, args: &[FormatArg<'_>]) -> io::Result<usize>;
}

// indentFormatter 对应 Go 的同名结构体；writer 字段模拟内嵌 io.Writer。
// indentLevel 保留 Go 的 int 语义，负缩进在原注释中已声明为未定义行为。
/// 带缩进层级与行首状态的 Formatter 实现体。
pub struct indentFormatter<W: Write> {
    writer: W,
    indent: Vec<u8>,
    indentLevel: i32,
    state: i32,
}

// replace 对应 Go 的 map[rune]string；用匹配函数保留相同转义表。
/// 将需转义字符映射为 Go `OutputFormat` 使用的转义串。
fn replace(old: char) -> Option<&'static str> {
    match old {
        '\0' => Some("\\0"),
        '\'' => Some("''"),
        '\n' => Some("\\n"),
        '\r' => Some("\\r"),
        '\\' => Some("\\\\"),
        _ => None,
    }
}

/// 构造 `indentFormatter`：初始位于行首、缩进层级为 0。
fn new_indent_formatter<W: Write>(w: W, indent: &str) -> indentFormatter<W> {
    indentFormatter {
        writer: w,
        indent: indent.as_bytes().to_vec(),
        indentLevel: 0,
        state: stBOL,
    }
}

// IndentFormatter returns a new Formatter which interprets %i and %u in the
// Format() formats string as indent and unindent commands. The commands can
// nest. The Formatter writes to io.Writer 'w' and inserts one 'indent'
// string per current indent level value.
// Behaviour of commands reaching negative indent levels is undefined.
//	IndentFormatter(os.Stdout, "\t").Format("abc%d%%e%i\nx\ny\n%uz\n", 3)
// output:
//	abc3%e
//	    x
//	    y
//	z
// The Go quoted string literal form of the above is:
//	"abc%%e\n\tx\n\tx\nz\n"
// The commands can be scattered between separate invocations of Format(),
// i.e. the formatter keeps track of the indent level and knows if it is
// positioned on start of a line and should emit indentation(s).
// The same output as above can be produced by e.g.:
//	f := IndentFormatter(os.Stdout, " ")
//	f.Format("abc%d%%e%i\nx\n", 3)
//	f.Format("y\n%uz\n")
// IndentFormatter 创建带缩进状态的 Formatter；state 初始为行首，和 Go 构造函数一致。
pub fn IndentFormatter<W: Write + 'static>(w: W, indent: &str) -> Box<dyn Formatter> {
    Box::new(new_indent_formatter(w, indent))
}

impl<W: Write> Write for indentFormatter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl<W: Write> indentFormatter<W> {
    // format 对应 Go 的私有方法 (*indentFormatter).format。
    // flat 为 true 时，非零缩进层级下的换行会被压成空格；false 时按 indentLevel 插入缩进字节。
    fn format(&mut self, flat: bool, format: &str, args: &[FormatArg<'_>]) -> io::Result<usize> {
        let mut buf: Vec<u8> = Vec::new();
        // Go 代码按 len(format) 逐字节访问 format[i]，所以这里也用 bytes 而不是 chars。
        for c in format.bytes() {
            match self.state {
                st0 => match c {
                    b'\n' => {
                        // flat 模式下，只要当前处于嵌套层级，Go 会把换行改写为空格。
                        let cc = if flat && self.indentLevel != 0 {
                            b' '
                        } else {
                            c
                        };
                        buf.push(cc);
                        self.state = stBOL;
                    }
                    b'%' => {
                        // 进入百分号状态，等待下一个字节判断是缩进命令还是普通格式片段。
                        self.state = stPERC;
                    }
                    _ => buf.push(c),
                },
                stBOL => match c {
                    b'\n' => {
                        let cc = if flat && self.indentLevel != 0 {
                            b' '
                        } else {
                            c
                        };
                        buf.push(cc);
                    }
                    b'%' => {
                        // 行首看到 % 时需要延迟处理，因为 %i/%u 不应触发行首缩进输出。
                        self.state = stBOLPERC;
                    }
                    _ => {
                        if !flat {
                            // Go 版本用 for range f.indentLevel 重复写入缩进；负值行为仍按原注释视为未定义。
                            for _ in 0..self.indentLevel {
                                buf.extend_from_slice(&self.indent);
                            }
                        }
                        buf.push(c);
                        self.state = st0;
                    }
                },
                stBOLPERC => match c {
                    b'i' => {
                        self.indentLevel += 1;
                        self.state = stBOL;
                    }
                    b'u' => {
                        self.indentLevel -= 1;
                        self.state = stBOL;
                    }
                    _ => {
                        if !flat {
                            for _ in 0..self.indentLevel {
                                buf.extend_from_slice(&self.indent);
                            }
                        }
                        buf.push(b'%');
                        buf.push(c);
                        self.state = st0;
                    }
                },
                stPERC => match c {
                    b'i' => {
                        self.indentLevel += 1;
                        self.state = st0;
                    }
                    b'u' => {
                        self.indentLevel -= 1;
                        self.state = st0;
                    }
                    _ => {
                        // 非 %i/%u 的百分号片段原样交给后续格式化，对应 Go 中 append('%', c)。
                        buf.push(b'%');
                        buf.push(c);
                        self.state = st0;
                    }
                },
                _ => panic!("unexpected state"),
            }
        }
        match self.state {
            stPERC | stBOLPERC => {
                // Go 文件末尾若停在百分号状态，会补回字面量 '%'。
                buf.push(b'%');
            }
            _ => {}
        }

        // Go 的 fmt.Fprintf 接收运行期格式串；在写入前处理本模块使用的 %d/%s/%v 和 %%。
        let rendered = String::from_utf8_lossy(&buf);
        write_go_style(self, rendered.as_ref(), args)
    }
}

// write_go_style 对应 Go fmt.Fprintf(f, string(buf), args...) 的写入边界。
/// 将状态机展开后的格式串按 Go 风格占位符替换参数，再一次性写入底层 Writer。
fn write_go_style<W: Write + ?Sized>(
    writer: &mut W,
    rendered_format: &str,
    args: &[FormatArg<'_>],
) -> io::Result<usize> {
    let bytes = rendered_format.as_bytes();
    let mut rendered = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut arg_index = 0;

    while index < bytes.len() {
        // 非 `%` 或末尾孤立 `%`：原样拷贝。
        if bytes[index] != b'%' || index + 1 == bytes.len() {
            rendered.push(bytes[index]);
            index += 1;
            continue;
        }

        // `%%` 转义为一个字面量百分号。
        if bytes[index + 1] == b'%' {
            rendered.push(b'%');
            index += 2;
            continue;
        }

        let Some((spec, next_index)) = parse_format_spec(bytes, index + 1) else {
            // 无法解析的 `%...` 片段按字面量输出，避免静默吞掉。
            rendered.push(bytes[index]);
            index += 1;
            continue;
        };
        if arg_index >= args.len() {
            // 参数不足时保留原始格式片段，与宽松解析策略一致。
            rendered.extend_from_slice(&bytes[index..next_index]);
            index = next_index;
            continue;
        }

        let value = format_argument(args[arg_index], &spec);
        rendered.extend_from_slice(value.as_bytes());
        arg_index += 1;
        index = next_index;
    }

    // fmt.Fprintf renders first and calls the underlying Writer once. In particular,
    // a short write with no error remains a short successful result.
    // 先完整渲染再单次 write，短写且无错误时仍返回已写长度（对齐 Go）。
    writer.write(&rendered)
}

/// 解析后的 printf 风格格式说明（标志、宽度、精度、动词）。
#[derive(Default)]
struct FormatSpec {
    left_align: bool,
    zero_pad: bool,
    alternate: bool,
    plus_sign: bool,
    space_sign: bool,
    width: Option<usize>,
    precision: Option<usize>,
    verb: u8,
}

/// 从 `%` 后的字节解析 `FormatSpec`；不支持的动词返回 `None`。
fn parse_format_spec(bytes: &[u8], mut index: usize) -> Option<(FormatSpec, usize)> {
    let mut spec = FormatSpec::default();
    // 先消费标志位：左对齐、零填充、`#`、`+`、空格符号。
    while index < bytes.len() {
        match bytes[index] {
            b'-' => spec.left_align = true,
            b'0' => spec.zero_pad = true,
            b'#' => spec.alternate = true,
            b'+' => spec.plus_sign = true,
            b' ' => spec.space_sign = true,
            _ => break,
        }
        index += 1;
    }

    let width_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index > width_start {
        spec.width = std::str::from_utf8(&bytes[width_start..index])
            .ok()
            .and_then(|width| width.parse().ok());
    }

    // `.` 后跟精度；单独的 `.` 表示精度为 0。
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        let precision_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        spec.precision = if index == precision_start {
            Some(0)
        } else {
            std::str::from_utf8(&bytes[precision_start..index])
                .ok()
                .and_then(|precision| precision.parse().ok())
        };
    }

    // 仅支持本模块迁移所需的一组动词。
    if index >= bytes.len() || !matches!(bytes[index], b'd' | b's' | b'v' | b'x' | b'X' | b'f') {
        return None;
    }
    spec.verb = bytes[index];
    Some((spec, index + 1))
}

/// 按动词与标志把单个参数格式化为字符串。
fn format_argument(arg: FormatArg<'_>, spec: &FormatSpec) -> String {
    let displayed = arg.to_string();
    let mut value = match spec.verb {
        b'd' => displayed
            .parse::<i128>()
            .map(|number| number.to_string())
            .unwrap_or(displayed),
        b'x' | b'X' => format_integer_hex(&displayed, spec.verb == b'X', spec.alternate),
        b'f' => displayed
            .parse::<f64>()
            .map(|number| format!("{:.*}", spec.precision.unwrap_or(6), number))
            .unwrap_or(displayed),
        // `%s` 可用精度截断字符数。
        b's' => spec
            .precision
            .map(|precision| displayed.chars().take(precision).collect())
            .unwrap_or(displayed),
        _ => displayed,
    };

    // Go 将整数精度解释为最少数字位数，符号与进制前缀不计入精度。
    if matches!(spec.verb, b'd' | b'x' | b'X')
        && let Some(precision) = spec.precision
    {
        value = apply_integer_precision(value, precision);
    }

    // `%d`/`%f` 在非负时应用 `+` 或空格符号标志。
    if matches!(spec.verb, b'd' | b'f') && !value.starts_with('-') {
        if spec.plus_sign {
            value.insert(0, '+');
        } else if spec.space_sign {
            value.insert(0, ' ');
        }
    }
    apply_width(value, spec)
}

/// 返回符号及 `0x`/`0X` 前缀结束处的字节下标。
fn numeric_prefix_len(value: &str) -> usize {
    let sign_len = value
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'-' | b'+' | b' ')) as usize;
    let prefix_len = value
        .get(sign_len..)
        .is_some_and(|rest| rest.starts_with("0x") || rest.starts_with("0X"))
        .then_some(2)
        .unwrap_or(0);
    sign_len + prefix_len
}

/// 按 Go 整数精度在符号/进制前缀之后补零。
fn apply_integer_precision(value: String, precision: usize) -> String {
    let prefix_len = numeric_prefix_len(&value);
    let digits = &value[prefix_len..];
    // Go 的整数格式化特例：精度为零且数值为零时不输出数字。
    if precision == 0 && digits == "0" {
        return value[..prefix_len].to_owned();
    }
    let padding = precision.saturating_sub(digits.len());
    if padding == 0 {
        return value;
    }
    format!("{}{}{}", &value[..prefix_len], "0".repeat(padding), digits)
}

/// 将整数按十六进制格式化，可选 `0x`/`0X` 前缀。
fn format_integer_hex(displayed: &str, uppercase: bool, alternate: bool) -> String {
    let Ok(number) = displayed.parse::<i128>() else {
        return displayed.to_owned();
    };
    let negative = number < 0;
    let magnitude = number.unsigned_abs();
    let digits = if uppercase {
        format!("{magnitude:X}")
    } else {
        format!("{magnitude:x}")
    };
    let prefix = if alternate {
        if uppercase { "0X" } else { "0x" }
    } else {
        ""
    };
    format!("{}{}{}", if negative { "-" } else { "" }, prefix, digits)
}

/// 按宽度与左对齐/零填充标志对已格式化字符串补齐。
fn apply_width(value: String, spec: &FormatSpec) -> String {
    let Some(width) = spec.width else {
        return value;
    };
    let padding = width.saturating_sub(value.chars().count());
    if padding == 0 {
        return value;
    }
    if spec.left_align {
        return format!("{value}{}", " ".repeat(padding));
    }
    // 整数指定精度时 Go 忽略 `0` 标志；否则零位于符号和进制前缀之后。
    if spec.zero_pad && !(matches!(spec.verb, b'd' | b'x' | b'X') && spec.precision.is_some()) {
        let prefix_len = numeric_prefix_len(&value);
        return format!(
            "{}{}{}",
            &value[..prefix_len],
            "0".repeat(padding),
            &value[prefix_len..]
        );
    }
    format!("{}{value}", " ".repeat(padding))
}

impl<W: Write> Formatter for indentFormatter<W> {
    // Format implements Format interface.
    // Format 对应 Go 的导出方法，固定使用非 flat 模式。
    fn Format(&mut self, format: &str, args: &[FormatArg<'_>]) -> io::Result<usize> {
        self.format(false, format, args)
    }
}

// flatFormatter 对应 Go 的 `type flatFormatter indentFormatter`。
// Rust 用一元组结构包住 indentFormatter，表达“同样状态、不同 Format 行为”的 Go 语义。
/// 扁平化 Formatter：复用缩进状态机，但以 flat 模式改写嵌套换行。
pub struct flatFormatter<W: Write>(indentFormatter<W>);

// FlatFormatter returns a newly created Formatter with the same functionality as the one returned
// by IndentFormatter except it allows a newline in the 'format' string argument of Format
// to pass through if the indent level is current zero.
// If the indent level is non-zero then such new lines are changed to a space character.
// There is no indent string, the %i and %u format verbs are used solely to determine the indent level.
// The FlatFormatter is intended for flattening of normally nested structure textual representation to
// a one top level structure per line form.
//	FlatFormatter(os.Stdout, " ").Format("abc%d%%e%i\nx\ny\n%uz\n", 3)
// output in the form of a Go quoted string literal:
//	"abc3%%e x y z\n"
// FlatFormatter 创建扁平化 Formatter；它沿用 indentFormatter 的状态机，但缩进字符串固定为空。
pub fn FlatFormatter<W: Write + 'static>(w: W) -> Box<dyn Formatter> {
    Box::new(flatFormatter(new_indent_formatter(w, "")))
}

impl<W: Write> Write for flatFormatter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<W: Write> Formatter for flatFormatter<W> {
    // Format implements Format interface.
    // Format 对应 Go 的 (*flatFormatter).Format，通过 flat=true 复用 indentFormatter.format。
    fn Format(&mut self, format: &str, args: &[FormatArg<'_>]) -> io::Result<usize> {
        self.0.format(true, format, args)
    }
}

// OutputFormat output escape character with backslash.
// OutputFormat 按 Go 的 replace 表转义字符串，未命中的 Unicode 字符保持原样写入。
pub fn OutputFormat(s: &str) -> String {
    // Go 版本使用 bytes.Buffer；用 String 累积同样的文本输出。
    let mut buf = String::new();
    for old in s.chars() {
        if let Some(new_val) = replace(old) {
            buf.push_str(new_val);
            continue;
        }
        buf.push(old);
    }

    buf
}
