// Copyright 2019 PingCAP, Inc.
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

// SQL dump 行解析：BlockParser 缓冲读取 + ChunkParser 词法驱动的 VALUES 行解析。
//
// Datum/Row/Chunk 描述单元格、行与导入分片（按字节偏移切分）。EscapeFlavor 控制
// MySQL 反斜杠转义；Parser trait 统一 CSV 与 SQL 解析器接口。ReadChunks 按最小
// 尺寸切分并行导入单元。

use crate::{
    CsvConfig, FileInfo, MakePooledReader, MydumpError, PooledReader, ReadSeekCloser, Storage,
    WorkerPool,
};
use std::fmt;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
/// 本模块错误别名，等同于 `MydumpError`。
pub type Error = MydumpError;
/// 读块缓冲相对请求 size 的放大倍数。
pub const BUFFER_SIZE_SCALE: i64 = 2;

#[derive(Clone, Debug, PartialEq)]
/// 单元格取值：NULL、整数、字节串或二进制字面量。
pub enum Datum {
    Null,
    I64(i64),
    Bytes(Vec<u8>),
    Binary(Vec<u8>),
}
#[derive(Clone, Debug, Default, PartialEq)]
/// 一行解析结果：列值、行号与占用字节长度。
pub struct Row {
    pub row: Vec<Datum>,
    pub row_id: i64,
    pub length: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 导入分片：起止偏移、真实偏移与行号区间，可选列名快照。
pub struct Chunk {
    pub offset: i64,
    pub end_offset: i64,
    pub real_offset: i64,
    pub prev_row_id_max: i64,
    pub row_id_max: i64,
    pub columns: Vec<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 字符串转义方言：关闭、MySQL 反斜杠、或带 NULL 特殊处理的 MySQL。
pub enum EscapeFlavor {
    None,
    MySql,
    MySqlWithNull,
}

/// 按块从 PooledReader 读入缓冲，维护解析游标与行对象池。
pub struct BlockParser {
    pub reader: PooledReader,
    pub buf: Vec<u8>,
    pub block_buf: Vec<u8>,
    pub is_last_chunk: bool,
    pub columns: Vec<String>,
    pub last_row: Row,
    pub pos: i64,
    pub check_row_len: bool,
    pub row_start_pos: i64,
    pub logger: crate::Logger,
    row_pool: Vec<Vec<Datum>>,
}
/// 构造 BlockParser：包装 reader，分配 size*BUFFER_SIZE_SCALE 的块缓冲。
pub fn makeBlockParser(
    reader: Box<dyn ReadSeekCloser>,
    size: i64,
    workers: Option<Arc<WorkerPool>>,
) -> BlockParser {
    BlockParser {
        reader: MakePooledReader(reader, workers),
        buf: Vec::new(),
        block_buf: vec![0; (size.max(1) * BUFFER_SIZE_SCALE) as usize],
        is_last_chunk: false,
        columns: Vec::new(),
        last_row: Row::default(),
        pos: 0,
        check_row_len: false,
        row_start_pos: 0,
        logger: crate::Logger::default(),
        row_pool: Vec::new(),
    }
}
/// `makeBlockParser` 的 snake_case 别名。
pub fn make_block_parser(
    reader: Box<dyn ReadSeekCloser>,
    size: i64,
    workers: Option<Arc<WorkerPool>>,
) -> BlockParser {
    makeBlockParser(reader, size, workers)
}
impl BlockParser {
    /// 开始统计当前行占用的字节长度。
    pub fn beginRowLenCheck(&mut self) {
        self.check_row_len = true;
        self.row_start_pos = self.pos
    }
    /// 结束行长度统计。
    pub fn endRowLenCheck(&mut self) {
        self.check_row_len = false
    }
    /// Seek 到指定字节偏移并重置缓冲，同步 row_id。
    pub fn SetPos(&mut self, pos: i64, row_id: i64) -> Result<(), Error> {
        let actual = self.reader.seek(SeekFrom::Start(pos as u64))? as i64;
        if actual != pos {
            return Err(Error::Io(format!(
                "set pos failed, required {pos}, got {actual}"
            )));
        }
        self.buf.clear();
        self.is_last_chunk = false;
        self.pos = pos;
        self.last_row.row_id = row_id;
        Ok(())
    }
    /// 返回底层 reader 当前流位置。
    pub fn ScannedPos(&mut self) -> Result<i64, Error> {
        Ok(self.reader.stream_position()? as i64)
    }
    /// 返回逻辑解析位置与当前行号。
    pub fn Pos(&self) -> (i64, i64) {
        (self.pos, self.last_row.row_id)
    }
    /// 关闭底层 pooled reader。
    pub fn Close(&mut self) -> Result<(), Error> {
        self.reader.Close()?;
        Ok(())
    }
    /// 当前列名列表。
    pub fn Columns(&self) -> &[String] {
        &self.columns
    }
    /// 设置列名。
    pub fn SetColumns(&mut self, c: Vec<String>) {
        self.columns = c
    }
    /// 设置当前行号。
    pub fn SetRowID(&mut self, id: i64) {
        self.last_row.row_id = id
    }
    /// 返回最近一次成功解析的行副本。
    pub fn LastRow(&self) -> Row {
        self.last_row.clone()
    }
    /// 清空行向量并放回对象池以复用。
    pub fn RecycleRow(&mut self, row: Row) {
        let mut values = row.row;
        values.clear();
        self.row_pool.push(values);
    }
    /// 从对象池取出 Datum 向量，空则新建。
    pub fn acquireDatumSlice(&mut self) -> Vec<Datum> {
        self.row_pool
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(16))
    }
    /// 将缓冲前缀记入日志，辅助定位语法错误。
    pub fn log_syntax_error(&self) {
        let content = &self.buf[..self.buf.len().min(256)];
        self.logger.error(format!(
            "syntax error at offset {}: {}",
            self.pos,
            String::from_utf8_lossy(content)
        ));
    }
    /// 替换日志器。
    pub fn SetLogger(&mut self, logger: crate::Logger) {
        self.logger = logger;
    }
    /// 从 reader 读下一块；文件头 UTF-8 BOM 在首次读入时剥离。
    pub fn read_block(&mut self) -> Result<(), Error> {
        let mut block = vec![0; self.block_buf.len()];
        let n = self.reader.read(&mut block)?;
        if n == 0 {
            self.is_last_chunk = true;
            return Ok(());
        }
        block.truncate(n);
        // 跳过 UTF-8 BOM（EF BB BF），避免污染首个 token。
        if self.pos == 0 && self.buf.is_empty() && block.starts_with(&[0xef, 0xbb, 0xbf]) {
            block.drain(..3);
            self.pos += 3
        }
        self.buf.extend(block);
        Ok(())
    }
}

/// 统一行解析器接口：定位、读行、列名与行对象回收。
pub trait Parser {
    fn Pos(&self) -> (i64, i64);
    fn SetPos(&mut self, pos: i64, row: i64) -> Result<(), Error>;
    fn ScannedPos(&mut self) -> Result<i64, Error>;
    fn Close(&mut self) -> Result<(), Error>;
    fn ReadRow(&mut self) -> Result<(), Error>;
    fn LastRow(&self) -> Row;
    fn RecycleRow(&mut self, row: Row);
    fn Columns(&self) -> &[String];
    fn SetColumns(&mut self, c: Vec<String>);
    fn SetRowID(&mut self, id: i64);
}
/// 基于 BlockParser + 生成词法器的 SQL INSERT VALUES 解析器。
pub struct ChunkParser {
    pub block_parser: BlockParser,
    pub esc_flavor: EscapeFlavor,
}
/// 构造 ChunkParser；`no_backslash_escapes` 为真时关闭反斜杠转义。
pub fn NewChunkParser(
    reader: Box<dyn ReadSeekCloser>,
    size: i64,
    workers: Option<Arc<WorkerPool>>,
    no_backslash_escapes: bool,
) -> ChunkParser {
    ChunkParser {
        block_parser: makeBlockParser(reader, size, workers),
        esc_flavor: if no_backslash_escapes {
            EscapeFlavor::None
        } else {
            EscapeFlavor::MySql
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 词法 token：行括号、关键字、字面量种类等。
pub enum Token {
    Nil,
    RowBegin,
    RowEnd,
    Values,
    Null,
    True,
    False,
    HexString,
    BinString,
    Integer,
    SingleQuoted,
    DoubleQuoted,
    BackQuoted,
    Unquoted,
}
/// 以 Debug 风格显示 token 名。
impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}
/// 展开引号内转义：双定界符与可选的反斜杠序列（\\0/\\n/...）。
pub fn unescapeString(input: &[u8], delimiter: u8, flavor: EscapeFlavor, escape: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == delimiter && i + 1 < input.len() && input[i + 1] == delimiter {
            out.push(delimiter);
            i += 2;
            continue;
        }
        if flavor != EscapeFlavor::None && input[i] == escape && i + 1 < input.len() {
            i += 1;
            out.push(match input[i] {
                b'0' => 0,
                b'b' => 8,
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'Z' => 26,
                b => b,
            });
            i += 1;
            continue;
        }
        out.push(input[i]);
        i += 1
    }
    out
}
impl ChunkParser {
    /// 词法驱动读一行 VALUES：括号嵌套、NULL/布尔/进制/引号字段。
    pub fn ReadRow(&mut self) -> Result<(), Error> {
        if self.block_parser.buf.is_empty() && !self.block_parser.is_last_chunk {
            self.block_parser.read_block()?
        }
        #[derive(Clone, Copy, Eq, PartialEq)]
        enum State {
            TableName,
            Columns,
            Values,
            Row,
        }

        let mut state = State::Values;
        let mut values = Vec::new();
        let mut row_length = 0_u64;
        loop {
            let (token, raw) = match crate::parser_generated::lex(self) {
                Ok(result) => result,
                Err(Error::Eof) if state != State::Values => {
                    return Err(Error::Syntax(format!(
                        "syntax error: premature EOF at offset {}",
                        self.block_parser.pos
                    )));
                }
                Err(error) => return Err(error),
            };
            row_length += raw.len() as u64;
            match state {
                State::TableName => match token {
                    Token::RowBegin => state = State::Columns,
                    Token::Values => state = State::Values,
                    Token::Unquoted | Token::DoubleQuoted | Token::BackQuoted => {}
                    _ => {
                        return Err(unexpected_token(
                            token,
                            &raw,
                            self.block_parser.pos,
                            "table name",
                        ));
                    }
                },
                State::Columns => match token {
                    Token::RowEnd => state = State::Values,
                    Token::Unquoted | Token::DoubleQuoted | Token::BackQuoted => {
                        self.block_parser
                            .columns
                            .push(decode_text(&raw, self.esc_flavor).to_ascii_lowercase());
                    }
                    _ => {
                        return Err(unexpected_token(
                            token,
                            &raw,
                            self.block_parser.pos,
                            "column list",
                        ));
                    }
                },
                State::Values => match token {
                    Token::RowBegin => {
                        self.block_parser.last_row.row_id += 1;
                        values = self.block_parser.acquireDatumSlice();
                        state = State::Row;
                    }
                    Token::Unquoted | Token::DoubleQuoted | Token::BackQuoted => {
                        self.block_parser.columns.clear();
                        state = State::TableName;
                    }
                    Token::Values => {}
                    _ => {
                        return Err(unexpected_token(
                            token,
                            &raw,
                            self.block_parser.pos,
                            "start of row",
                        ));
                    }
                },
                State::Row => {
                    let value = match token {
                        Token::RowEnd => {
                            self.block_parser.last_row.row = values;
                            self.block_parser.last_row.length = row_length;
                            return Ok(());
                        }
                        Token::Null => Datum::Null,
                        Token::True => Datum::I64(1),
                        Token::False => Datum::I64(0),
                        Token::Integer => match raw_str(&raw)?.parse::<i64>() {
                            Ok(integer) => Datum::I64(integer),
                            Err(_) => Datum::Bytes(raw),
                        },
                        Token::HexString => Datum::Binary(parse_based(&raw, 16)?),
                        Token::BinString => Datum::Binary(parse_based(&raw, 2)?),
                        Token::Unquoted | Token::SingleQuoted | Token::DoubleQuoted => {
                            Datum::Bytes(decode_text(&raw, self.esc_flavor).into_bytes())
                        }
                        _ => {
                            return Err(unexpected_token(
                                token,
                                &raw,
                                self.block_parser.pos,
                                "data literal",
                            ));
                        }
                    };
                    values.push(value);
                }
            }
        }
    }
}

fn decode_text(raw: &[u8], flavor: EscapeFlavor) -> String {
    let quote = raw.first().copied();
    let quoted = quote.is_some_and(|q| matches!(q, b'\'' | b'"' | b'`')) && raw.len() >= 2;
    let body = if quoted { &raw[1..raw.len() - 1] } else { raw };
    let effective_flavor = if quote == Some(b'`') {
        EscapeFlavor::None
    } else {
        flavor
    };
    String::from_utf8_lossy(&unescapeString(
        body,
        quote.unwrap_or(0),
        effective_flavor,
        b'\\',
    ))
    .into_owned()
}

fn unexpected_token(token: Token, raw: &[u8], pos: i64, expected: &str) -> Error {
    Error::Syntax(format!(
        "syntax error: unexpected {token} ({}) at offset {pos}, expecting {expected}",
        String::from_utf8_lossy(raw)
    ))
}
/// 将 token 原始字节转为 UTF-8 字符串切片。
fn raw_str(raw: &[u8]) -> Result<&str, Error> {
    std::str::from_utf8(raw).map_err(|e| Error::Syntax(e.to_string()))
}
/// 解析 0x/0b 或 x''/b'' 形式的进制字面量为字节向量。
fn parse_based(raw: &[u8], radix: u32) -> Result<Vec<u8>, Error> {
    let raw = raw_str(raw)?;
    let text = if raw.len() >= 3
        && matches!(raw.as_bytes()[0], b'x' | b'X' | b'b' | b'B')
        && raw.as_bytes()[1] == b'\''
        && raw.ends_with('\'')
    {
        &raw[2..raw.len() - 1]
    } else {
        raw.trim_start_matches("0x")
            .trim_start_matches("0X")
            .trim_start_matches("0b")
            .trim_start_matches("0B")
    };
    if radix == 16 {
        hex::decode(if text.len() % 2 == 0 {
            text.to_owned()
        } else {
            format!("0{text}")
        })
        .map_err(|e| Error::Syntax(e.to_string()))
    } else {
        let mut out = Vec::new();
        let mut byte = 0u8;
        for (i, c) in text.bytes().rev().enumerate() {
            if c == b'1' {
                byte |= 1 << (i % 8)
            }
            if i % 8 == 7 {
                out.push(byte);
                byte = 0
            }
        }
        if text.len() % 8 != 0 {
            out.push(byte)
        }
        out.reverse();
        Ok(out)
    }
}
/// 将 ChunkParser 委托到内部 BlockParser 实现 Parser。
impl Parser for ChunkParser {
    fn Pos(&self) -> (i64, i64) {
        self.block_parser.Pos()
    }
    fn SetPos(&mut self, p: i64, r: i64) -> Result<(), Error> {
        self.block_parser.SetPos(p, r)
    }
    fn ScannedPos(&mut self) -> Result<i64, Error> {
        self.block_parser.ScannedPos()
    }
    fn Close(&mut self) -> Result<(), Error> {
        self.block_parser.Close()
    }
    fn ReadRow(&mut self) -> Result<(), Error> {
        ChunkParser::ReadRow(self)
    }
    fn LastRow(&self) -> Row {
        self.block_parser.LastRow()
    }
    fn RecycleRow(&mut self, r: Row) {
        self.block_parser.RecycleRow(r)
    }
    fn Columns(&self) -> &[String] {
        self.block_parser.Columns()
    }
    fn SetColumns(&mut self, c: Vec<String>) {
        self.block_parser.SetColumns(c)
    }
    fn SetRowID(&mut self, id: i64) {
        self.block_parser.SetRowID(id)
    }
}
/// 按最小字节尺寸切分已解析行为多个 Chunk，供并行导入。
pub fn ReadChunks(parser: &mut dyn Parser, min_size: i64) -> Result<Vec<Chunk>, Error> {
    let (mut offset, mut row) = parser.Pos();
    let mut chunks = Vec::new();
    loop {
        match parser.ReadRow() {
            Ok(()) => {
                let (pos, id) = parser.Pos();
                // 累计跨度达到 min_size 时切出一个 Chunk。
                if pos - offset >= min_size {
                    chunks.push(Chunk {
                        offset,
                        end_offset: pos,
                        real_offset: offset,
                        prev_row_id_max: row,
                        row_id_max: id,
                        columns: parser.Columns().to_vec(),
                    });
                    offset = pos;
                    row = id
                }
            }
            Err(Error::Eof) => {
                let (pos, id) = parser.Pos();
                if pos > offset {
                    chunks.push(Chunk {
                        offset,
                        end_offset: pos,
                        real_offset: offset,
                        prev_row_id_max: row,
                        row_id_max: id,
                        columns: parser.Columns().to_vec(),
                    })
                }
                break;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(chunks)
}
/// 持续 ReadRow 直到逻辑位置达到 target。
pub fn ReadUntil(parser: &mut dyn Parser, target: i64) -> Result<(), Error> {
    while parser.Pos().0 < target {
        match parser.ReadRow() {
            Ok(()) => {}
            Err(Error::Eof) => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
/// `ReadChunks` 的 snake_case 别名。
pub fn read_chunks(p: &mut dyn Parser, s: i64) -> Result<Vec<Chunk>, Error> {
    ReadChunks(p, s)
}
/// `ReadUntil` 的 snake_case 别名。
pub fn read_until(p: &mut dyn Parser, t: i64) -> Result<(), Error> {
    ReadUntil(p, t)
}
/// 分配默认容量的 Datum 向量（无池化的独立入口）。
pub fn acquireDatumSlice() -> Vec<Datum> {
    Vec::with_capacity(16)
}
/// 转发到 BlockParser::log_syntax_error。
pub fn logSyntaxError(parser: &BlockParser) {
    parser.log_syntax_error()
}
/// 转发到 BlockParser::read_block。
pub fn readBlock(parser: &mut BlockParser) -> Result<(), Error> {
    parser.read_block()
}
/// 转发设置日志器。
pub fn SetLogger(parser: &mut BlockParser, logger: crate::Logger) {
    parser.SetLogger(logger)
}
/// Token 的字符串表示。
pub fn String(token: Token) -> String {
    token.to_string()
}
/// `unescapeString` 别名。
pub fn unescape(input: &[u8], delimiter: u8, flavor: EscapeFlavor, escape: u8) -> Vec<u8> {
    unescapeString(input, delimiter, flavor, escape)
}
/// 按 FileInfo 来源类型打开 CSV 或 SQL ChunkParser。
pub fn OpenReader(
    file: &FileInfo,
    cfg: &CsvConfig,
    store: &dyn Storage,
) -> Result<Box<dyn Parser>, Error> {
    let mut data = Vec::new();
    store
        .open(&file.file_meta.path, file.file_meta.compression)?
        .read_to_end(&mut data)?;
    let reader: Box<dyn ReadSeekCloser> = Box::new(crate::StringReader::from_bytes(data));
    match file.file_meta.source_type {
        crate::SourceType::Csv => Ok(Box::new(crate::NewCSVParser(
            cfg, reader, cfg.header, None,
        )?)),
        crate::SourceType::Sql => Ok(Box::new(NewChunkParser(reader, 64 * 1024, None, false))),
        other => Err(Error::Configuration(format!("no row parser for {other}"))),
    }
}
