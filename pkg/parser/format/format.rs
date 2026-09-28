// Copyright (c) 2014 The sortutil Authors. All rights reserved.
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

// SQL 文本格式化与 AST 恢复（restore）辅助模块。
//
// 本模块提供两类能力：
// 1. **缩进/扁平格式化器**：在写入过程中解释 `%i`（增加缩进）与 `%u`（减少缩进）
//    特殊命令，并兼容类似 `printf` 的参数替换，用于生成带层级的 SQL 文本；
// 2. **Restore 上下文**：把抽象语法树（AST，Abstract Syntax Tree，解析后的结构化
//    SQL 表示）按可配置标志写回 SQL 字符串，控制关键字大小写、字符串/标识符
//    引号风格、TiDB 特殊注释等行为。
//
// 术语说明：
// - **恢复（restore）**：从 AST 重新生成等价 SQL 文本的过程；
// - **CTE（Common Table Expression，公用表表达式）**：`WITH` 子句定义的临时命名结果集；
// - **Placement Rule（放置规则）**：TiDB/TiKV 中控制数据副本放置位置的策略。

// 本文件对照 pkg/parser/format/format.go，保留格式化状态机、恢复标志和写入上下文的顺序。

use std::fmt::Display;
use std::io::{self, Write};
use std::ops::{BitOr, BitOrAssign};

/// 格式化状态机：普通正文状态（非行首、未遇到 `%`）。
const ST0: u8 = 0;
/// 格式化状态机：行首（Beginning Of Line）状态，下次非换行字符前可能先写入缩进。
const ST_BOL: u8 = 1;
/// 格式化状态机：刚读到 `%`，等待判断是 `%i`/`%u` 还是普通百分号转义。
const ST_PERC: u8 = 2;
/// 格式化状态机：行首刚读到 `%`，缩进命令处理完后仍保持行首语义。
const ST_BOL_PERC: u8 = 3;

/// 扩展的写入器接口：在普通 `Write` 之上增加带缩进命令的 `Format`。
///
/// 对应 Go 的 `Formatter`；格式串中的 `%i`/`%u` 由实现方解释为增减缩进，
/// 其余 `%` 动词交给类 `printf` 渲染逻辑，参数以 `Display` trait 对象切片传入。
// Formatter 对应 Go 的扩展 io.Writer 接口；format 额外解释 %i/%u 缩进命令。
// 可变 any 参数在这里表达为 Display trait object 切片。
pub trait Formatter: Write {
    /// 按格式串写入文本；返回写入的字节数。
    #[allow(non_snake_case)]
    fn Format(&mut self, format: &str, args: &[&dyn Display]) -> io::Result<usize>;
}

/// 带缩进的格式化器状态，跨多次 `Format` 调用保留缩进级别与行首状态。
// IndentFormatterState 对应 Go 的 indentFormatter，跨多次调用保存缩进级别和行首状态。
pub struct IndentFormatterState<W: Write> {
    /// 底层字节写入目标。
    writer: W,
    /// 每一级缩进使用的原始字节串（如制表符）。
    indent: Vec<u8>,
    /// 当前缩进层级；仅正数时在行首实际展开缩进。
    indent_level: i32,
    /// 四态状态机当前状态（`ST0` / `ST_BOL` / `ST_PERC` / `ST_BOL_PERC`）。
    state: u8,
}

/// 近似 `fmt.Fprintf` 的参数替换：跳过已被状态机消费的 `%i`/`%u`，
/// 将 `%%` 写成 `%`，其余动词按出现顺序（或 `%[n]` 显式下标）取 `Display` 参数，
/// 并处理宽度、对齐、零填充与精度。
// render_printf_shape 保留 fmt.Fprintf 的常用逐参数替换形状。
// %i/%u 已在状态机中消费；%% 输出百分号，其余格式动词按出现顺序取 Display 参数。
fn render_printf_shape(format: &[u8], args: &[&dyn Display]) -> String {
    let mut output = String::new();
    let mut arg_index = 0;
    let mut index = 0;
    while index < format.len() {
        let Some(relative_percent) = format[index..].iter().position(|byte| *byte == b'%') else {
            output.push_str(&String::from_utf8_lossy(&format[index..]));
            break;
        };
        let percent = index + relative_percent;
        output.push_str(&String::from_utf8_lossy(&format[index..percent]));
        if percent + 1 == format.len() {
            output.push('%');
            break;
        }
        if format[percent + 1] == b'%' {
            output.push('%');
            index = percent + 2;
            continue;
        }

        let mut verb_end = percent + 1;
        while verb_end < format.len() && !format[verb_end].is_ascii_alphabetic() {
            verb_end += 1;
        }
        if verb_end == format.len() {
            output.push_str(&String::from_utf8_lossy(&format[percent..]));
            break;
        }

        // 解析可选的 `%[n]` 显式参数下标；未指定则按出现顺序递增 `arg_index`。
        let options = &format[percent + 1..verb_end];
        let selected_index = options
            .strip_prefix(b"[")
            .and_then(|rest| {
                rest.iter()
                    .position(|byte| *byte == b']')
                    .map(|end| &rest[..end])
            })
            .and_then(|digits| std::str::from_utf8(digits).ok())
            .and_then(|digits| digits.parse::<usize>().ok())
            .and_then(|one_based| one_based.checked_sub(1));
        let current_index = selected_index.unwrap_or(arg_index);
        if selected_index.is_none() {
            arg_index += 1;
        }

        let Some(arg) = args.get(current_index) else {
            output.push_str(&String::from_utf8_lossy(&format[percent..=verb_end]));
            index = verb_end + 1;
            continue;
        };
        // 从动词前的选项中提取对齐、零填充、宽度与精度，再对渲染结果做填充/截断。
        let mut rendered = arg.to_string();
        let option_text = String::from_utf8_lossy(options);
        let flags_and_width = option_text
            .rsplit_once(']')
            .map_or(option_text.as_ref(), |(_, suffix)| suffix);
        let left_aligned = flags_and_width.contains('-');
        let zero_padded = flags_and_width.starts_with('0');
        let width = flags_and_width
            .trim_start_matches(&['-', '+', ' ', '0', '#'][..])
            .split('.')
            .next()
            .and_then(|digits| digits.parse::<usize>().ok())
            .unwrap_or(0);
        let precision = flags_and_width
            .split_once('.')
            .and_then(|(_, digits)| digits.parse::<usize>().ok());

        if let Some(precision) = precision {
            if format[verb_end] == b's' {
                rendered = rendered.chars().take(precision).collect();
            } else if format[verb_end] == b'd' {
                let (sign, digits) = rendered
                    .strip_prefix('-')
                    .map_or(("", rendered.as_str()), |digits| ("-", digits));
                rendered = format!("{sign}{digits:0>precision$}");
            }
        }

        let rendered_width = rendered.chars().count();
        if width > rendered_width {
            let padding = width - rendered_width;
            if left_aligned {
                rendered.extend(std::iter::repeat_n(' ', padding));
            } else if zero_padded {
                if let Some(digits) = rendered.strip_prefix('-') {
                    rendered = format!("-{}{digits}", "0".repeat(padding));
                } else {
                    rendered = format!("{}{rendered}", "0".repeat(padding));
                }
            } else {
                rendered = format!("{}{rendered}", " ".repeat(padding));
            }
        }
        output.push_str(&rendered);
        index = verb_end + 1;
    }
    output
}

impl<W: Write> IndentFormatterState<W> {
    /// 逐字节跑四态状态机，再把缓冲交给 `render_printf_shape` 做参数替换并写出。
    /// `flat` 为真时，在非零缩进层级把换行压成空格（扁平输出）。
    // format_inner 对应 Go (*indentFormatter).format：逐字节运行四态状态机。
    fn format_inner(
        &mut self,
        flat: bool,
        format: &str,
        args: &[&dyn Display],
    ) -> io::Result<usize> {
        let mut buffer = Vec::new();
        for &byte in format.as_bytes() {
            match self.state {
                ST0 => match byte {
                    b'\n' => {
                        buffer.push(if flat && self.indent_level != 0 {
                            b' '
                        } else {
                            byte
                        });
                        self.state = ST_BOL;
                    }
                    b'%' => self.state = ST_PERC,
                    _ => buffer.push(byte),
                },
                ST_BOL => match byte {
                    b'\n' => buffer.push(if flat && self.indent_level != 0 {
                        b' '
                    } else {
                        byte
                    }),
                    b'%' => self.state = ST_BOL_PERC,
                    _ => {
                        if !flat {
                            // Go 对负缩进级别行为未定义；这里只在正数时实际展开缩进。
                            for _ in 0..self.indent_level.max(0) {
                                buffer.extend_from_slice(&self.indent);
                            }
                        }
                        buffer.push(byte);
                        self.state = ST0;
                    }
                },
                ST_BOL_PERC => match byte {
                    b'i' => {
                        self.indent_level += 1;
                        self.state = ST_BOL;
                    }
                    b'u' => {
                        self.indent_level -= 1;
                        self.state = ST_BOL;
                    }
                    _ => {
                        if !flat {
                            for _ in 0..self.indent_level.max(0) {
                                buffer.extend_from_slice(&self.indent);
                            }
                        }
                        buffer.extend_from_slice(&[b'%', byte]);
                        self.state = ST0;
                    }
                },
                ST_PERC => match byte {
                    b'i' => {
                        self.indent_level += 1;
                        self.state = ST0;
                    }
                    b'u' => {
                        self.indent_level -= 1;
                        self.state = ST0;
                    }
                    _ => {
                        buffer.extend_from_slice(&[b'%', byte]);
                        self.state = ST0;
                    }
                },
                _ => panic!("unexpected state"),
            }
        }

        // Go 会把调用末尾悬空的百分号交给 fmt.Fprintf；状态本身留到下次调用继续跟踪行首。
        if matches!(self.state, ST_PERC | ST_BOL_PERC) {
            buffer.push(b'%');
        }
        let rendered = render_printf_shape(&buffer, args);
        // Go `fmt.Fprintf` renders once and returns the underlying writer's
        // actual write count, including a short write without an error.
        self.writer.write(rendered.as_bytes())
    }
}

impl<W: Write> Write for IndentFormatterState<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl<W: Write> Formatter for IndentFormatterState<W> {
    #[allow(non_snake_case)]
    fn Format(&mut self, format: &str, args: &[&dyn Display]) -> io::Result<usize> {
        self.format_inner(false, format, args)
    }
}

/// 构造缩进格式化器：初始状态为行首，每级缩进使用 `indent` 的字节内容。
// IndentFormatter 对应 Go 构造函数：初始处于行首，每级使用调用方给定字节串缩进。
#[allow(non_snake_case)]
pub fn IndentFormatter<W: Write>(writer: W, indent: &str) -> IndentFormatterState<W> {
    IndentFormatterState {
        writer,
        indent: indent.as_bytes().to_vec(),
        indent_level: 0,
        state: ST_BOL,
    }
}

/// 扁平格式化器：内部复用 `IndentFormatterState`，在非零缩进时把换行压成空格。
// FlatFormatterState 对应 Go 的 flatFormatter 新类型，共用同一状态但在非零缩进时把换行压成空格。
pub struct FlatFormatterState<W: Write>(IndentFormatterState<W>);

impl<W: Write> Write for FlatFormatterState<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<W: Write> Formatter for FlatFormatterState<W> {
    #[allow(non_snake_case)]
    fn Format(&mut self, format: &str, args: &[&dyn Display]) -> io::Result<usize> {
        self.0.format_inner(true, format, args)
    }
}

/// 构造扁平格式化器；空缩进串仍跟踪层级，以决定换行是否应被压平。
// FlatFormatter 对应 Go 构造函数；空缩进串只保留层级，以决定换行是否压平。
#[allow(non_snake_case)]
pub fn FlatFormatter<W: Write>(writer: W) -> FlatFormatterState<W> {
    FlatFormatterState(IndentFormatter(writer, ""))
}

/// 对输出字符串做基础转义：NUL→`\0`、单引号→`''`、换行→`\n`、回车→`\r`。
// OutputFormat 对应 Go 的字符转义函数，依次处理 NUL、单引号、换行和回车。
#[allow(non_snake_case)]
pub fn OutputFormat(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for old in input.chars() {
        match old {
            '\0' => output.push_str("\\0"),
            '\'' => output.push_str("''"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            _ => output.push(old),
        }
    }
    output
}

/// AST 恢复时的行为标志位集（`u64`）。
///
/// 互斥组（如单引号/双引号字符串）由调用方组合；查询方法中左侧标志通常优先。
// RestoreFlags 对应 Go 的 uint64 位集；互斥组仍由调用方负责选择，左侧标志具有更高判断优先级。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RestoreFlags(pub u64);

impl BitOr for RestoreFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for RestoreFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// 字符串字面量使用单引号包裹，内部单引号写成 `''`。
#[allow(non_upper_case_globals)]
pub const RestoreStringSingleQuotes: RestoreFlags = RestoreFlags(1 << 0);
/// 字符串字面量使用双引号包裹，内部双引号写成 `""`。
#[allow(non_upper_case_globals)]
pub const RestoreStringDoubleQuotes: RestoreFlags = RestoreFlags(1 << 1);
/// 写入字符串前将反斜线转义为 `\\`。
#[allow(non_upper_case_globals)]
pub const RestoreStringEscapeBackslash: RestoreFlags = RestoreFlags(1 << 2);
/// 关键字输出为大写；与小写标志并存时大写优先。
#[allow(non_upper_case_globals)]
pub const RestoreKeyWordUppercase: RestoreFlags = RestoreFlags(1 << 3);
/// 关键字输出为小写（仅在未设大写标志时生效）。
#[allow(non_upper_case_globals)]
pub const RestoreKeyWordLowercase: RestoreFlags = RestoreFlags(1 << 4);
/// 标识符（库/表/列名等）输出为大写。
#[allow(non_upper_case_globals)]
pub const RestoreNameUppercase: RestoreFlags = RestoreFlags(1 << 5);
/// 标识符输出为小写（仅在未设大写标志时生效）。
#[allow(non_upper_case_globals)]
pub const RestoreNameLowercase: RestoreFlags = RestoreFlags(1 << 6);
/// 标识符用双引号包裹，内部双引号成对转义。
#[allow(non_upper_case_globals)]
pub const RestoreNameDoubleQuotes: RestoreFlags = RestoreFlags(1 << 7);
/// 标识符用反引号包裹，内部反引号写成 `` ` ` ``。
#[allow(non_upper_case_globals)]
pub const RestoreNameBackQuotes: RestoreFlags = RestoreFlags(1 << 8);
/// 二元运算符两侧写入空格。
#[allow(non_upper_case_globals)]
pub const RestoreSpacesAroundBinaryOperation: RestoreFlags = RestoreFlags(1 << 9);
/// 为二元运算表达式外加括号。
#[allow(non_upper_case_globals)]
pub const RestoreBracketAroundBinaryOperation: RestoreFlags = RestoreFlags(1 << 10);
/// 恢复字符串时省略字符集前缀。
#[allow(non_upper_case_globals)]
pub const RestoreStringWithoutCharset: RestoreFlags = RestoreFlags(1 << 11);
/// 默认字符集场景下省略字符集前缀。
#[allow(non_upper_case_globals)]
pub const RestoreStringWithoutDefaultCharset: RestoreFlags = RestoreFlags(1 << 12);
/// 启用 TiDB 特殊注释包装：`/*T![feature] ... */`。
#[allow(non_upper_case_globals)]
pub const RestoreTiDBSpecialComment: RestoreFlags = RestoreFlags(1 << 13);
/// 恢复时跳过 Placement Rule（数据放置规则）相关子句。
#[allow(non_upper_case_globals)]
pub const SkipPlacementRuleForRestore: RestoreFlags = RestoreFlags(1 << 14);
/// 恢复 TTL（Time To Live，存活时间）相关定义时强制关闭 enable。
#[allow(non_upper_case_globals)]
pub const RestoreWithTTLEnableOff: RestoreFlags = RestoreFlags(1 << 15);
/// 恢复对象名时省略 schema（数据库）名前缀。
#[allow(non_upper_case_globals)]
pub const RestoreWithoutSchemaName: RestoreFlags = RestoreFlags(1 << 16);
/// 恢复列等对象名时省略表名前缀。
#[allow(non_upper_case_globals)]
pub const RestoreWithoutTableName: RestoreFlags = RestoreFlags(1 << 17);
/// 面向非预处理语句计划缓存场景的恢复变体。
#[allow(non_upper_case_globals)]
pub const RestoreForNonPrepPlanCache: RestoreFlags = RestoreFlags(1 << 18);
/// 为 `BETWEEN` 表达式外加括号。
#[allow(non_upper_case_globals)]
pub const RestoreBracketAroundBetweenExpr: RestoreFlags = RestoreFlags(1 << 19);
/// 允许在规范化恢复路径中省略不影响语义的冗余括号。
// RestoreSkipRedundantParentheses 允许规范化路径省略不影响 SQL 语义的冗余括号。
#[allow(non_upper_case_globals)]
pub const RestoreSkipRedundantParentheses: RestoreFlags = RestoreFlags(1 << 20);

/// 默认恢复标志组合：单引号字符串 + 大写关键字 + 反引号标识符。
// DefaultRestoreFlags 对应 Go 默认组合：单引号字符串、大写关键字、反引号名称。
#[allow(non_upper_case_globals)]
pub const DefaultRestoreFlags: RestoreFlags =
    RestoreFlags(RestoreStringSingleQuotes.0 | RestoreKeyWordUppercase.0 | RestoreNameBackQuotes.0);

impl RestoreFlags {
    /// 判断是否设置了指定标志位。
    fn has(self, flag: RestoreFlags) -> bool {
        self.0 & flag.0 != 0
    }

    // 以下查询方法逐一对应 Go 的导出 flag 检查器。
    /// 是否省略 schema 名。
    #[allow(non_snake_case)]
    pub fn HasWithoutSchemaNameFlag(self) -> bool {
        self.has(RestoreWithoutSchemaName)
    }
    /// 是否省略表名。
    #[allow(non_snake_case)]
    pub fn HasWithoutTableNameFlag(self) -> bool {
        self.has(RestoreWithoutTableName)
    }
    /// 是否使用单引号字符串。
    #[allow(non_snake_case)]
    pub fn HasStringSingleQuotesFlag(self) -> bool {
        self.has(RestoreStringSingleQuotes)
    }
    /// 是否使用双引号字符串。
    #[allow(non_snake_case)]
    pub fn HasStringDoubleQuotesFlag(self) -> bool {
        self.has(RestoreStringDoubleQuotes)
    }
    /// 是否转义反斜线。
    #[allow(non_snake_case)]
    pub fn HasStringEscapeBackslashFlag(self) -> bool {
        self.has(RestoreStringEscapeBackslash)
    }
    /// 是否大写关键字。
    #[allow(non_snake_case)]
    pub fn HasKeyWordUppercaseFlag(self) -> bool {
        self.has(RestoreKeyWordUppercase)
    }
    /// 是否小写关键字。
    #[allow(non_snake_case)]
    pub fn HasKeyWordLowercaseFlag(self) -> bool {
        self.has(RestoreKeyWordLowercase)
    }
    /// 是否大写标识符。
    #[allow(non_snake_case)]
    pub fn HasNameUppercaseFlag(self) -> bool {
        self.has(RestoreNameUppercase)
    }
    /// 是否小写标识符。
    #[allow(non_snake_case)]
    pub fn HasNameLowercaseFlag(self) -> bool {
        self.has(RestoreNameLowercase)
    }
    /// 是否双引号包裹标识符。
    #[allow(non_snake_case)]
    pub fn HasNameDoubleQuotesFlag(self) -> bool {
        self.has(RestoreNameDoubleQuotes)
    }
    /// 是否反引号包裹标识符。
    #[allow(non_snake_case)]
    pub fn HasNameBackQuotesFlag(self) -> bool {
        self.has(RestoreNameBackQuotes)
    }
    /// 二元运算两侧是否加空格。
    #[allow(non_snake_case)]
    pub fn HasSpacesAroundBinaryOperationFlag(self) -> bool {
        self.has(RestoreSpacesAroundBinaryOperation)
    }
    /// 二元运算是否外加括号。
    #[allow(non_snake_case)]
    pub fn HasRestoreBracketAroundBinaryOperation(self) -> bool {
        self.has(RestoreBracketAroundBinaryOperation)
    }
    /// 是否省略默认字符集前缀。
    #[allow(non_snake_case)]
    pub fn HasStringWithoutDefaultCharset(self) -> bool {
        self.has(RestoreStringWithoutDefaultCharset)
    }
    /// `BETWEEN` 表达式是否外加括号。
    #[allow(non_snake_case)]
    pub fn HasRestoreBracketAroundBetweenExpr(self) -> bool {
        self.has(RestoreBracketAroundBetweenExpr)
    }
    /// 是否允许省略冗余括号。
    #[allow(non_snake_case)]
    pub fn HasRestoreSkipRedundantParentheses(self) -> bool {
        self.has(RestoreSkipRedundantParentheses)
    }
    /// 是否省略字符集前缀。
    #[allow(non_snake_case)]
    pub fn HasStringWithoutCharset(self) -> bool {
        self.has(RestoreStringWithoutCharset)
    }
    /// 是否启用 TiDB 特殊注释。
    #[allow(non_snake_case)]
    pub fn HasTiDBSpecialCommentFlag(self) -> bool {
        self.has(RestoreTiDBSpecialComment)
    }
    /// 是否跳过 Placement Rule。
    #[allow(non_snake_case)]
    pub fn HasSkipPlacementRuleForRestoreFlag(self) -> bool {
        self.has(SkipPlacementRuleForRestore)
    }
    /// 是否强制 TTL enable 为 off。
    #[allow(non_snake_case)]
    pub fn HasRestoreWithTTLEnableOff(self) -> bool {
        self.has(RestoreWithTTLEnableOff)
    }
    /// 是否使用非预处理计划缓存恢复模式。
    #[allow(non_snake_case)]
    pub fn HasRestoreForNonPrepPlanCache(self) -> bool {
        self.has(RestoreForNonPrepPlanCache)
    }
}

/// 恢复写入目标：在 `Write` 之上提供按字符串写入的便捷方法。
// RestoreWriter 对应 Go 的 io.Writer + io.StringWriter 组合接口。
pub trait RestoreWriter: Write {
    /// 将 UTF-8 文本按字节写入；默认实现委托给 `write`。
    fn write_string(&mut self, text: &str) -> io::Result<usize> {
        self.write(text.as_bytes())
    }
}

impl<T: Write + ?Sized> RestoreWriter for T {}

/// AST 恢复上下文：持有标志、输出目标、默认库名以及表达式父子关系状态。
// RestoreCtx 对应 Go 的恢复上下文，保存标志、输出目标、默认库以及表达式父节点状态。
pub struct RestoreCtx<'a> {
    /// 当前恢复使用的标志位集。
    pub Flags: RestoreFlags,
    /// 恢复文本的写入目标。
    pub In: &'a mut dyn RestoreWriter,
    /// 默认数据库名，用于省略与默认库相同的 schema 前缀等场景。
    pub DefaultDB: String,
    /// 父二元运算符类型编码；`0` 表示当前不在二元运算子树中。
    // ParentBinaryOp 为 0 表示无父二元操作；调用方在子节点完成后必须恢复旧值。
    pub ParentBinaryOp: i32,
    /// 当前子表达式相对父二元运算符的左右侧标记。
    // ParentBinarySide 标记当前子表达式处在父操作符左侧还是右侧。
    pub ParentBinarySide: i32,
    /// 当前是否位于一元运算操作数的恢复路径中。
    // InUnaryOperation 标记当前恢复路径位于一元操作数内部。
    pub InUnaryOperation: bool,
    /// CTE 名称栈，用于判断表名是否来自当前 `WITH` 作用域。
    pub CTERestorer: CTERestorer,
}

/// 创建恢复上下文；表达式相关字段以零值初始化。
// NewRestoreCtx 对应 Go 构造函数，所有表达式上下文状态使用零值初始化。
#[allow(non_snake_case)]
pub fn NewRestoreCtx<'a>(flags: RestoreFlags, writer: &'a mut dyn RestoreWriter) -> RestoreCtx<'a> {
    RestoreCtx {
        Flags: flags,
        In: writer,
        DefaultDB: String::new(),
        ParentBinaryOp: 0,
        ParentBinarySide: 0,
        InUnaryOperation: false,
        CTERestorer: CTERestorer::default(),
    }
}

impl RestoreCtx<'_> {
    /// 按标志写入关键字；大写标志优先于小写标志。
    // WriteKeyWord 按恢复标志调整关键字大小写；大写标志优先于小写标志。
    #[allow(non_snake_case)]
    pub fn WriteKeyWord(&mut self, keyword: &str) -> io::Result<()> {
        let rendered = if self.Flags.HasKeyWordUppercaseFlag() {
            keyword.to_uppercase()
        } else if self.Flags.HasKeyWordLowercaseFlag() {
            keyword.to_lowercase()
        } else {
            keyword.to_owned()
        };
        self.In.write_string(&rendered).map(|_| ())
    }

    /// 在启用特殊注释标志时，用 `/*T![feature_id] ... */` 包装回调写入的内容。
    // WriteWithSpecialComments 对应 Go 的回调包装：未启用标志时直接执行，启用时写入 /*T![feature] ... */。
    #[allow(non_snake_case)]
    pub fn WriteWithSpecialComments<F>(&mut self, feature_id: &str, body: F) -> io::Result<()>
    where
        F: FnOnce(&mut Self) -> io::Result<()>,
    {
        if !self.Flags.HasTiDBSpecialCommentFlag() {
            return body(self);
        }
        self.WritePlain("/*T!")?;
        if !feature_id.is_empty() {
            self.WritePlain(&format!("[{}]", feature_id))?;
        }
        self.WritePlain(" ")?;
        // 与 Go 一致：回调失败立即返回，不补写结尾，保留部分输出供上层观察错误。
        body(self)?;
        self.WritePlain(" */")
    }

    /// 用特殊注释包装并写入单个关键字。
    // WriteKeyWordWithSpecialComments 用特殊注释包装单个关键字。
    #[allow(non_snake_case)]
    pub fn WriteKeyWordWithSpecialComments(
        &mut self,
        feature_id: &str,
        keyword: &str,
    ) -> io::Result<()> {
        self.WriteWithSpecialComments(feature_id, |ctx| ctx.WriteKeyWord(keyword))
    }

    /// 按标志转义并写入字符串字面量（含可选的首尾引号）。
    // WriteString 按标志转义反斜线与引号，并在需要时补同类首尾引号。
    #[allow(non_snake_case)]
    pub fn WriteString(&mut self, value: &str) -> io::Result<()> {
        let mut value = value.to_owned();
        if self.Flags.HasStringEscapeBackslashFlag() {
            value = value.replace('\\', "\\\\");
        }
        let quotes = if self.Flags.HasStringSingleQuotesFlag() {
            value = value.replace('\'', "''");
            "'"
        } else if self.Flags.HasStringDoubleQuotesFlag() {
            value = value.replace('"', "\"\"");
            "\""
        } else {
            ""
        };
        self.In.write_string(quotes)?;
        self.In.write_string(&value)?;
        self.In.write_string(quotes).map(|_| ())
    }

    /// 按标志转换大小写并写入标识符（可选双引号/反引号包裹）。
    // WriteName 按名称标志转换大小写，并使用双引号或反引号包裹、成对转义内部定界符。
    #[allow(non_snake_case)]
    pub fn WriteName(&mut self, name: &str) -> io::Result<()> {
        let mut name = if self.Flags.HasNameUppercaseFlag() {
            name.to_uppercase()
        } else if self.Flags.HasNameLowercaseFlag() {
            name.to_lowercase()
        } else {
            name.to_owned()
        };
        let quotes = if self.Flags.HasNameDoubleQuotesFlag() {
            name = name.replace('"', "\"\"");
            "\""
        } else if self.Flags.HasNameBackQuotesFlag() {
            name = name.replace('`', "``");
            "`"
        } else {
            ""
        };
        self.In.write_string(quotes)?;
        self.In.write_string(&name)?;
        self.In.write_string(quotes).map(|_| ())
    }

    /// 原样写入纯文本，不做大小写、引号或转义处理。
    // WritePlain 直接写入文本，不进行大小写、引用或转义处理。
    #[allow(non_snake_case)]
    pub fn WritePlain(&mut self, plain_text: &str) -> io::Result<()> {
        self.In.write_string(plain_text).map(|_| ())
    }

    /// 按类 `printf` 格式串渲染后原样写入。
    // WritePlainf 对应 fmt.Fprintf；动态格式参数沿用本文件的逐参数渲染器。
    #[allow(non_snake_case)]
    pub fn WritePlainf(&mut self, format: &str, args: &[&dyn Display]) -> io::Result<()> {
        let rendered = render_printf_shape(format.as_bytes(), args);
        self.In.write_string(&rendered).map(|_| ())
    }
}

/// CTE（公用表表达式）名称栈：记录当前 `WITH` 作用域内已声明的临时表名。
// CTERestorer 对应 Go 的 CTE 名称栈，用于判断表名是否来自当前 WITH 子句。
#[derive(Default)]
pub struct CTERestorer {
    /// 当前作用域内已记录的 CTE 名称列表（区分大小写）。
    pub CTENames: Vec<String>,
}

impl CTERestorer {
    /// 判断名称是否在当前 CTE 栈中（精确、区分大小写）。
    // IsCTETableName 对应 slices.Contains，保持区分大小写的精确比较。
    #[allow(non_snake_case)]
    pub fn IsCTETableName(&self, name_l: &str) -> bool {
        self.CTENames.iter().any(|name| name == name_l)
    }

    /// 将 CTE 名称追加到当前作用域末尾。
    // RecordCTEName 将新名称追加到当前恢复作用域末尾。
    #[allow(non_snake_case)]
    pub fn RecordCTEName(&mut self, name_l: &str) {
        self.CTENames.push(name_l.to_owned());
    }

    /// 返回作用域清理闭包：记录当前栈长，退出时截断本作用域新增的名称。
    // RestoreCTEFunc 对应 Go 返回的清理闭包：记录当前长度，并在作用域退出时截断新增名称。
    // Rust 闭包显式接收可变引用，避免长期独占借用阻止调用方在作用域内继续记录 CTE。
    #[allow(non_snake_case)]
    pub fn RestoreCTEFunc(&self) -> impl FnOnce(&mut CTERestorer) + 'static {
        let length = self.CTENames.len();
        move |restorer| restorer.CTENames.truncate(length)
    }
}
