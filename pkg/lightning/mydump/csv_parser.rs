// Copyright 2020 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// CSV 行解析器：按可配置分隔符/引号/转义读取 mydump 或通用 CSV。
//
// 支持表头、行前缀（lines_starting_by）、空行、尾部分隔符裁剪、字符集转换，
// 以及最大字段长度限制。实现 `Parser` trait，供 Lightning 统一按行导入。

use crate::{CharsetConvertor, Datum, Error, Parser, ReadSeekCloser, Row};
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicUsize, Ordering};

/// 单个 CSV 字段允许的最大字节数（默认约 120MiB），超限报配置错误。
pub static LargestEntryLimit: AtomicUsize = AtomicUsize::new(120 * 1024 * 1024);
/// 引号字段未正确闭合时的语法错误文案。
const ERR_UNTERMINATED_QUOTED_FIELD: &str = "syntax error: unterminated quoted field";
/// 转义符后无后续字符时的语法错误文案。
const ERR_DANGLING_BACKSLASH: &str = "syntax error: no character after backslash";
/// 未转义引号出现在非引号字段中，或相邻字段缺少分隔符。
const ERR_UNEXPECTED_QUOTE_FIELD: &str =
    "syntax error: cannot have consecutive fields without separator";

#[derive(Clone, Debug)]
/// CSV 方言配置：分隔符、引号、行结束、转义、NULL 表示与行为开关。
pub struct CsvConfig {
    pub fields_terminated_by: String,
    pub fields_enclosed_by: String,
    pub lines_terminated_by: String,
    pub lines_starting_by: String,
    pub fields_escaped_by: String,
    pub null: String,
    pub header: bool,
    pub trim_last_separators: bool,
    pub allow_empty_line: bool,
    pub quoted_null_is_text: bool,
    pub unescaped_quote: bool,
}
/// 默认接近 MySQL LOAD DATA：逗号分隔、双引号、`\\N` 表示 NULL。
impl Default for CsvConfig {
    fn default() -> Self {
        Self {
            fields_terminated_by: ",".into(),
            fields_enclosed_by: "\"".into(),
            lines_terminated_by: "\n".into(),
            lines_starting_by: String::new(),
            fields_escaped_by: "\\".into(),
            null: "\\N".into(),
            header: false,
            trim_last_separators: false,
            allow_empty_line: false,
            quoted_null_is_text: false,
            unescaped_quote: true,
        }
    }
}
#[derive(Clone, Debug, Default)]
/// 解析得到的单个字段：原始字节、是否曾被引号包裹、是否视为 NULL。
pub struct Field {
    content: Vec<u8>,
    quoted: bool,
    is_null: bool,
}
/// 流式 CSV 解析器状态：缓冲、游标、列名、上一行与配置派生的分隔字节序列。
pub struct CsvParser {
    data: Vec<u8>,
    pos: usize,
    row_start: usize,
    row_id: i64,
    columns: Vec<String>,
    last_row: Row,
    cfg: CsvConfig,
    comma: Vec<u8>,
    quote: Vec<u8>,
    newline: Vec<u8>,
    starting: Vec<u8>,
    escape: Option<u8>,
    convertor: Option<CharsetConvertor>,
    header_pending: bool,
    recycled_rows: Vec<Vec<Datum>>,
}

/// 将配置中的分隔/引号/行结束符按字符集编码器转为字节序列。
pub fn encodeSpecialSymbols(
    cfg: &CsvConfig,
    convertor: &CharsetConvertor,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), Error> {
    Ok((
        convertor.Encode(&cfg.fields_terminated_by)?,
        convertor.Encode(&cfg.fields_enclosed_by)?,
        convertor.Encode(&cfg.lines_terminated_by)?,
    ))
}
/// 读取全部输入并构造 CsvParser；可选字符集转换器会编码特殊符号。
pub fn NewCSVParser(
    cfg: &CsvConfig,
    mut reader: Box<dyn ReadSeekCloser>,
    should_parse_header: bool,
    convertor: Option<CharsetConvertor>,
) -> Result<CsvParser, Error> {
    let mut data = Vec::new();
    reader.seek(SeekFrom::Start(0))?;
    reader.read_to_end(&mut data)?;
    let (comma, quote, newline) = if let Some(c) = &convertor {
        encodeSpecialSymbols(cfg, c)?
    } else {
        (
            cfg.fields_terminated_by.as_bytes().to_vec(),
            cfg.fields_enclosed_by.as_bytes().to_vec(),
            cfg.lines_terminated_by.as_bytes().to_vec(),
        )
    };
    if comma.is_empty() {
        return Err(Error::Configuration(
            "CSV field terminator cannot be empty".into(),
        ));
    }
    if !cfg.lines_starting_by.is_empty()
        && !newline.is_empty()
        && cfg
            .lines_starting_by
            .as_bytes()
            .windows(newline.len())
            .any(|window| window == newline)
    {
        return Err(Error::Configuration(format!(
            "STARTING BY '{}' cannot contain LINES TERMINATED BY '{}'",
            cfg.lines_starting_by, cfg.lines_terminated_by
        )));
    }
    Ok(CsvParser {
        data,
        pos: 0,
        row_start: 0,
        row_id: 0,
        columns: Vec::new(),
        last_row: Row::default(),
        cfg: cfg.clone(),
        comma,
        quote,
        newline,
        starting: cfg.lines_starting_by.as_bytes().to_vec(),
        escape: cfg.fields_escaped_by.as_bytes().first().copied(),
        convertor,
        header_pending: should_parse_header,
        recycled_rows: Vec::new(),
    })
}
/// `NewCSVParser` 的 snake_case 别名。
pub fn new_csv_parser(
    cfg: &CsvConfig,
    reader: Box<dyn ReadSeekCloser>,
    header: bool,
    c: Option<CharsetConvertor>,
) -> Result<CsvParser, Error> {
    NewCSVParser(cfg, reader, header, c)
}

impl CsvParser {
    /// 读取并解析表头行为列名（去空白与反引号），随后关闭 header_pending。
    pub fn ReadColumns(&mut self) -> Result<Vec<String>, Error> {
        let fields = self.readRecord()?;
        let mut columns = Vec::with_capacity(fields.len());
        for f in fields {
            let decoded = self.decode(&f.content)?;
            columns.push(decoded.to_lowercase())
        }
        self.columns = columns.clone();
        self.header_pending = false;
        Ok(columns)
    }
    /// 读下一数据行填入 last_row；若仍待解析表头则先消费表头。
    pub fn ReadRow(&mut self) -> Result<(), Error> {
        self.row_id += 1;
        if self.header_pending {
            self.ReadColumns()?;
        }
        let fields = self.readRecord()?;
        let row_length = fields.iter().map(|field| field.content.len() as u64).sum();
        let mut row = self.recycled_rows.pop().unwrap_or_default();
        row.clear();
        row.reserve(fields.len().saturating_sub(row.capacity()));
        for field in fields {
            let decoded = self.decode(&field.content)?;
            // 未加引号的 NULL 标记，或关闭 quoted_null_is_text 时，写入 Datum::Null。
            if field.is_null && (!field.quoted || !self.cfg.quoted_null_is_text) {
                row.push(Datum::Null)
            } else {
                row.push(Datum::Bytes(decoded.into_bytes()))
            }
        }
        self.last_row = Row {
            row,
            row_id: self.row_id,
            length: row_length,
        };
        Ok(())
    }
    /// 按可选 CharsetConvertor 解码字段字节，否则按 UTF-8。
    fn decode(&self, value: &[u8]) -> Result<String, Error> {
        if let Some(c) = &self.convertor {
            c.Decode(value)
        } else {
            String::from_utf8(value.to_vec()).map_err(|e| Error::Encoding(e.to_string()))
        }
    }
    /// 读完整一行字段列表：处理行前缀、空行、分隔符与尾部分隔符裁剪。
    pub fn readRecord(&mut self) -> Result<Vec<Field>, Error> {
        'record: loop {
            if self.pos >= self.data.len() {
                return Err(Error::Eof);
            }
            let line_start = self.pos;
            if !self.starting.is_empty() {
                let physical_end = self
                    .next_newline_offset()
                    .unwrap_or(self.data.len() - self.pos);
                if let Some(offset) = find_subslice(
                    &self.data[self.pos..self.pos + physical_end],
                    &self.starting,
                ) {
                    self.pos += offset + self.starting.len();
                } else {
                    self.skip_to_newline();
                    continue;
                }
            }
            self.row_start = self.pos;
            let mut fields = Vec::new();
            if self.starts_with_newline() {
                self.consume_newline();
                if self.cfg.allow_empty_line {
                    return Ok(vec![Field::default()]);
                }
                continue;
            }
            loop {
                let field = self.read_field()?;
                fields.push(field);
                if self.pos >= self.data.len() {
                    return Ok(fields);
                }
                if self.starts_with_newline() {
                    self.consume_newline();
                    if !self.cfg.allow_empty_line
                        && fields.len() == 1
                        && !fields[0].quoted
                        && fields[0].content.iter().all(u8::is_ascii_whitespace)
                    {
                        continue 'record;
                    }
                    return Ok(fields);
                }
                if self.data[self.pos..].starts_with(&self.comma) {
                    self.pos += self.comma.len();
                    if self.cfg.trim_last_separators && self.starts_with_newline() {
                        self.consume_newline();
                        return Ok(fields);
                    }
                    continue;
                }
                if self.pos == line_start {
                    return Err(Error::Syntax(ERR_UNEXPECTED_QUOTE_FIELD.into()));
                }
                return Err(Error::Syntax(format!(
                    "unexpected byte at CSV offset {}",
                    self.pos
                )));
            }
        }
    }
    /// 读一个字段：引号字段支持转义与双引号转义；非引号字段扫到分隔符。
    fn read_field(&mut self) -> Result<Field, Error> {
        let field_start = self.pos;
        if !self.quote.is_empty() && self.data[self.pos..].starts_with(&self.quote) {
            self.pos += self.quote.len();
            let mut out = Vec::new();
            loop {
                if self.pos >= self.data.len() {
                    return Err(Error::Syntax(ERR_UNTERMINATED_QUOTED_FIELD.into()));
                }
                if self.data[self.pos..].starts_with(&self.quote) {
                    // 连续两个引号表示转义后的引号字面量。
                    if self
                        .data
                        .get(self.pos + self.quote.len()..)
                        .is_some_and(|r| r.starts_with(&self.quote))
                    {
                        out.extend_from_slice(&self.quote);
                        self.pos += self.quote.len() * 2;
                        continue;
                    }
                    let after_quote = self.pos + self.quote.len();
                    let closes_field = after_quote >= self.data.len()
                        || self.data[after_quote..].starts_with(&self.comma)
                        || self.is_newline_at(after_quote);
                    if self.cfg.unescaped_quote && !closes_field {
                        out.extend_from_slice(&self.quote);
                        self.pos = after_quote;
                        continue;
                    }
                    let is_null = self.data[field_start + self.quote.len()..self.pos]
                        == *self.cfg.null.as_bytes();
                    self.pos += self.quote.len();
                    return Ok(Field {
                        content: out,
                        quoted: true,
                        is_null,
                    });
                }
                if Some(self.data[self.pos]) == self.escape {
                    self.append_escape(&mut out)?
                } else {
                    out.push(self.data[self.pos]);
                    self.pos += 1
                }
                self.ensure_entry_limit(field_start)?;
            }
        }
        let start = self.pos;
        while self.pos < self.data.len()
            && !self.data[self.pos..].starts_with(&self.comma)
            && !self.starts_with_newline()
        {
            if !self.quote.is_empty()
                && self.data[self.pos..].starts_with(&self.quote)
                && !self.cfg.unescaped_quote
            {
                return Err(Error::Syntax(ERR_UNEXPECTED_QUOTE_FIELD.into()));
            }
            self.pos += 1;
            self.ensure_entry_limit(field_start)?;
        }
        let raw = self.data[start..self.pos].to_vec();
        let is_null = raw == self.cfg.null.as_bytes();
        Ok(Field {
            content: self.unescapeString(&raw)?,
            quoted: false,
            is_null,
        })
    }
    /// 若当前字段已超过 LargestEntryLimit 则返回配置错误。
    fn ensure_entry_limit(&self, _start: usize) -> Result<(), Error> {
        let limit = LargestEntryLimit.load(Ordering::Relaxed);
        if self.pos.saturating_sub(self.row_start) > limit {
            Err(Error::Configuration(
                "size of row cannot exceed the max value of txn-entry-size-limit".into(),
            ))
        } else {
            Ok(())
        }
    }
    /// 消费转义序列（\\0/\\n/\\t 等）并追加解码后的单字节。
    fn append_escape(&mut self, out: &mut Vec<u8>) -> Result<(), Error> {
        self.pos += 1;
        if self.pos >= self.data.len() {
            return Err(Error::Syntax(ERR_DANGLING_BACKSLASH.into()));
        }
        let value = match self.data[self.pos] {
            b'0' => 0,
            b'b' => 8,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'Z' => 26,
            b => b,
        };
        out.push(value);
        self.pos += 1;
        Ok(())
    }
    /// 对非引号字段内容做转义展开；无转义符时原样返回。
    pub fn unescapeString(&self, input: &[u8]) -> Result<Vec<u8>, Error> {
        let Some(escape) = self.escape else {
            return Ok(input.to_vec());
        };
        let mut out = Vec::new();
        let mut i = 0;
        while i < input.len() {
            if input[i] == escape {
                if i + 1 == input.len() {
                    return Err(Error::Syntax(ERR_DANGLING_BACKSLASH.into()));
                }
                i += 1;
                out.push(match input[i] {
                    b'0' => 0,
                    b'b' => 8,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'Z' => 26,
                    b => b,
                })
            } else {
                out.push(input[i])
            }
            i += 1
        }
        Ok(out)
    }
    /// 将游标推进到下一行结束符之后（或文件末尾）。
    fn skip_to_newline(&mut self) {
        if let Some(i) = self.next_newline_offset() {
            self.pos += i;
            self.consume_newline();
        } else {
            self.pos = self.data.len()
        }
    }

    fn next_newline_offset(&self) -> Option<usize> {
        if self.newline.is_empty() {
            self.data[self.pos..]
                .iter()
                .position(|byte| *byte == b'\r' || *byte == b'\n')
        } else {
            find_subslice(&self.data[self.pos..], &self.newline)
        }
    }

    fn starts_with_newline(&self) -> bool {
        self.is_newline_at(self.pos)
    }

    fn is_newline_at(&self, position: usize) -> bool {
        if self.newline.is_empty() {
            self.data
                .get(position)
                .is_some_and(|byte| *byte == b'\r' || *byte == b'\n')
        } else {
            self.data[position..].starts_with(&self.newline)
        }
    }

    fn consume_newline(&mut self) {
        self.pos += if self.newline.is_empty() {
            usize::from(self.pos < self.data.len())
        } else {
            self.newline.len()
        };
    }
    /// 跳到下一行结束；若已 EOF 则返回 Eof。
    pub fn ReadUntilTerminator(&mut self) -> Result<(), Error> {
        self.skip_to_newline();
        if self.pos >= self.data.len() {
            Err(Error::Eof)
        } else {
            Ok(())
        }
    }
    /// `ReadUntilTerminator` 的 camelCase 别名。
    pub fn readUntilTerminator(&mut self) -> Result<(), Error> {
        self.ReadUntilTerminator()
    }
    // The following helpers expose the same state transitions used by the Go parser.
    /// 窥视当前位置起 n 字节，不足则 None。
    pub fn peekBytes(&self, n: usize) -> Option<&[u8]> {
        self.data.get(self.pos..self.pos + n)
    }
    /// 读取并消费一个字节。
    pub fn readByte(&mut self) -> Result<u8, Error> {
        let b = *self.data.get(self.pos).ok_or(Error::Eof)?;
        self.pos += 1;
        Ok(b)
    }
    /// 向前跳过至多 n 字节。
    pub fn skipBytes(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.data.len())
    }
    /// 当前位置是否恰好匹配给定字节序列。
    pub fn tryPeekExact(&self, v: &[u8]) -> bool {
        self.data[self.pos..].starts_with(v)
    }
    /// 若匹配则消费该序列并返回 true。
    pub fn tryReadExact(&mut self, v: &[u8]) -> bool {
        if self.tryPeekExact(v) {
            self.pos += v.len();
            true
        } else {
            false
        }
    }
    /// 尝试消费字段分隔符。
    pub fn tryReadComma(&mut self) -> bool {
        let v = self.comma.clone();
        self.tryReadExact(&v)
    }
    /// 尝试消费行结束符。
    pub fn tryReadNewLine(&mut self) -> bool {
        let v = self.newline.clone();
        self.tryReadExact(&v)
    }
    /// 尝试消费起始引号定界符。
    pub fn tryReadOpenDelimiter(&mut self) -> bool {
        let v = self.quote.clone();
        !v.is_empty() && self.tryReadExact(&v)
    }
    /// 尝试消费结束引号（与开引号相同字节序列）。
    pub fn tryReadCloseDelimiter(&mut self) -> bool {
        self.tryReadOpenDelimiter()
    }
    /// 当前位置是否为转义字符。
    pub fn tryReadEscaped(&mut self) -> bool {
        self.escape
            .is_some_and(|e| self.data.get(self.pos) == Some(&e))
    }
    /// 将 Eof 错误替换为指定错误（用于把过早结束映射为语法错误）。
    pub fn replaceEOF<T>(&self, result: Result<T, Error>, replacement: Error) -> Result<T, Error> {
        match result {
            Err(Error::Eof) => Err(replacement),
            v => v,
        }
    }
    /// 将 token 追加到记录缓冲（对齐 Go 辅助函数）。
    pub fn appendCSVTokenToRecordBuffer(buffer: &mut Vec<u8>, token: &[u8]) {
        buffer.extend_from_slice(token)
    }
    /// 读一个字段并仅返回内容字节。
    pub fn readQuotedField(&mut self) -> Result<Vec<u8>, Error> {
        self.read_field().map(|f| f.content)
    }
    /// `readQuotedField` 别名。
    pub fn readQuotedToken(&mut self) -> Result<Vec<u8>, Error> {
        self.readQuotedField()
    }
    /// 读非引号或通用字段内容。
    pub fn readUnquoteToken(&mut self) -> Result<Vec<u8>, Error> {
        self.read_field().map(|f| f.content)
    }
    /// 读取直到 target 出现（不含 target），游标停在 target 起点。
    pub fn readUntil(&mut self, target: &[u8]) -> Result<Vec<u8>, Error> {
        let i = find_subslice(&self.data[self.pos..], target).ok_or(Error::Eof)?;
        let value = self.data[self.pos..self.pos + i].to_vec();
        self.pos += i;
        Ok(value)
    }
}
/// 在 data 中查找 needle 首次出现的下标。
fn find_subslice(data: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    data.windows(needle.len()).position(|w| w == needle)
}
/// 将 CsvParser 适配为统一的 Parser 接口（位置、读写行、列名回收等）。
impl Parser for CsvParser {
    fn Pos(&self) -> (i64, i64) {
        (self.pos as i64, self.row_id)
    }
    fn SetPos(&mut self, p: i64, r: i64) -> Result<(), Error> {
        if p < 0 || p as usize > self.data.len() {
            return Err(Error::Io("CSV seek out of range".into()));
        }
        self.pos = p as usize;
        self.row_id = r;
        Ok(())
    }
    fn ScannedPos(&mut self) -> Result<i64, Error> {
        Ok(self.pos as i64)
    }
    fn Close(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn ReadRow(&mut self) -> Result<(), Error> {
        CsvParser::ReadRow(self)
    }
    fn LastRow(&self) -> Row {
        self.last_row.clone()
    }
    fn RecycleRow(&mut self, row: Row) {
        let mut values = row.row;
        values.clear();
        self.recycled_rows.push(values);
    }
    fn Columns(&self) -> &[String] {
        &self.columns
    }
    fn SetColumns(&mut self, c: Vec<String>) {
        self.columns = c
    }
    fn SetRowID(&mut self, id: i64) {
        self.row_id = id
    }
}
/// 初始化探测：返回当前 LargestEntryLimit 值。
pub fn init() -> usize {
    LargestEntryLimit.load(Ordering::Relaxed)
}
