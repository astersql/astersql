// Copyright 2026 AsterSQL.
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

// `SELECT INTO OUTFILE` 执行器。
//
// 将子执行器产出的行按 MySQL 兼容的 FIELDS/LINES 格式写出本地文件。
// `SelectIntoSource` 抽象子执行器；`SelectIntoSink` 抽象写出目标（默认本地文件），
// Close 时按 Flush → Close sink → Close child 的优先级汇总错误。
use std::any::Any;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::sync::Arc;

use astersql_errors as errors;
use astersql_parser_ast as ast;
use astersql_types::datum as types;
use astersql_types::field::{ETDatetime, ETDuration, ETJson, ETString, ETTimestamp, EvalType};
use astersql_util_chunk as chunk;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

mod mysql {
    pub use astersql_parser_mysql::r#const::*;
    pub use astersql_parser_mysql::r#type::*;
}

/// 绝对值 ≥ 该阈值时浮点改用科学计数法输出。
const EXP_FORMAT_BIG: f64 = 1e15;
/// 绝对值 < 该阈值（且非 0）时浮点改用科学计数法输出。
const EXP_FORMAT_SMALL: f64 = 1e-15;

/// 透传给子执行器的上下文句柄（类型擦除）。
#[derive(Clone)]
pub struct SelectIntoContext(pub Arc<dyn Any + Send + Sync>);

impl Default for SelectIntoContext {
    fn default() -> Self {
        Self(Arc::new(()))
    }
}

/// OUTFILE 的字段/行分隔与转义选项（对应 `FIELDS` / `LINES` 子句）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LineFieldsInfo {
    /// 字段分隔符。
    pub FieldsTerminatedBy: String,
    /// 字段包围符。
    pub FieldsEnclosedBy: String,
    /// 转义字符。
    pub FieldsEscapedBy: String,
    /// 是否仅对字符串类可选包围。
    pub FieldsOptEnclosed: bool,
    /// 行前缀（当前写出路径未强制使用）。
    pub LinesStartingBy: String,
    /// 行终止符。
    pub LinesTerminatedBy: String,
}

/// Supplies the child executor operations that SELECT INTO needs. There are no
/// fallback implementations: adapters must preserve the real executor's
/// Open/Next/Close and affected-row behavior.
///
/// SELECT INTO 所需的子执行器操作边界；无占位实现，须保持真实 Open/Next/Close 与影响行数语义。
pub trait SelectIntoSource {
    /// 打开子执行器。
    fn Open(&mut self, ctx: SelectIntoContext) -> Result<(), errors::SharedError>;
    /// 拉取一批行到 chunk。
    fn Next(
        &mut self,
        ctx: SelectIntoContext,
        chunk: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError>;
    /// 关闭子执行器。
    fn Close(&mut self) -> Result<(), errors::SharedError>;
    /// 分配可复用的缓存 chunk。
    fn NewCacheChunk(&mut self) -> Box<chunk::Chunk>;
    /// 输出列的字段类型。
    fn FieldTypes(&self) -> Vec<types::FieldType>;
    /// 累计影响行数（写出成功后调用）。
    fn AddAffectedRows(&mut self, rows: u64);
}

/// Keeps write, flush, and close independently observable so Close can retain
/// Go's error priority even for injected or non-file sinks.
///
/// 写出目标抽象：Write/Flush/Close 可独立观测，以便 Close 保留 Go 的错误优先级。
pub trait SelectIntoSink {
    /// 写入全部字节。
    fn WriteAll(&mut self, data: &[u8]) -> io::Result<()>;
    /// 刷缓冲。
    fn Flush(&mut self) -> io::Result<()>;
    /// 关闭目标。
    fn Close(&mut self) -> io::Result<()>;
}

/// 本地 OUTFILE 落地：以 `O_CREATE|O_EXCL` 独占创建文件。
struct LocalOutfileSink {
    /// 缓冲写；`None` 表示已关闭。
    writer: Option<BufWriter<File>>,
}

impl LocalOutfileSink {
    /// 独占创建文件；Unix 下权限 0640（组可读，兼容 MySQL）。
    fn create_exclusive(path: &str) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o640);
        let file = options.open(path)?;
        Ok(Self {
            writer: Some(BufWriter::new(file)),
        })
    }

    /// 取得仍打开的写缓冲，已关闭则报错。
    fn writer(&mut self) -> io::Result<&mut BufWriter<File>> {
        self.writer
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "outfile is closed"))
    }
}

impl SelectIntoSink for LocalOutfileSink {
    fn WriteAll(&mut self, data: &[u8]) -> io::Result<()> {
        self.writer()?.write_all(data)
    }

    fn Flush(&mut self) -> io::Result<()> {
        self.writer()?.flush()
    }

    fn Close(&mut self) -> io::Result<()> {
        let Some(mut writer) = self.writer.take() else {
            return Ok(());
        };
        writer.flush()?;
        let file = writer.into_inner().map_err(|error| error.into_error())?;
        // std::fs::File closes on drop and cannot report that drop error. A final
        // sync keeps the close phase independently fallible like os.File.Close.
        file.sync_all()
    }
}

// SelectIntoExec represents a SelectInto executor.
/// `SELECT INTO OUTFILE` 执行器状态与缓冲。
pub struct SelectIntoExec {
    /// 子执行器数据源。
    pub source: Box<dyn SelectIntoSource>,
    /// AST 中的 INTO 选项（文件名、类型等）。
    pub intoOpt: Box<ast::SelectIntoOption>,
    /// 字段/行格式选项。
    pub LineFieldsInfo: LineFieldsInfo,

    /// 当前行组装缓冲。
    pub lineBuf: Vec<u8>,
    /// 浮点格式化临时缓冲。
    pub realBuf: Vec<u8>,
    /// 当前字段内容缓冲。
    pub fieldBuf: Vec<u8>,
    /// 转义后字段缓冲。
    pub escapeBuf: Vec<u8>,
    /// 当前字段是否处于包围模式（影响分隔符是否转义）。
    pub enclosed: bool,
    /// 写出目标；Open 后设置。
    pub writer: Option<Box<dyn SelectIntoSink>>,
    /// 子执行器行缓存 chunk。
    pub chk: Option<Box<chunk::Chunk>>,
    /// 列类型缓存。
    pub fieldTypes: Vec<types::FieldType>,
    /// 是否已成功 Open（决定 Close 是否做事）。
    pub started: bool,
}

impl SelectIntoExec {
    // Open implements the Executor Open interface.
    /// 打开：仅支持 Outfile；独占建文件后打开子执行器。
    pub fn Open(&mut self, ctx: SelectIntoContext) -> Result<(), errors::SharedError> {
        if self.intoOpt.Tp != ast::SelectIntoType::Outfile {
            return Err(errors::New("unsupported SelectInto type"));
        }

        // create_new maps to O_CREATE|O_EXCL. On Unix, mode(0640) preserves the
        // MySQL-compatible group-readable permission used by the Go executor.
        let sink = LocalOutfileSink::create_exclusive(&self.intoOpt.FileName)
            .map_err(|error| errors::New(error.to_string()))?;
        self.started = true;
        self.writer = Some(Box::new(sink));
        self.chk = Some(self.source.NewCacheChunk());
        self.fieldTypes = self.source.FieldTypes();
        self.lineBuf = Vec::with_capacity(1024);
        self.fieldBuf = Vec::with_capacity(64);
        self.escapeBuf = Vec::with_capacity(64);
        self.source.Open(ctx)
    }

    // Next implements the Executor Next interface.
    /// 循环拉取子执行器全部行并写出；自身不向 output 填行。
    pub fn Next(
        &mut self,
        ctx: SelectIntoContext,
        _output: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError> {
        loop {
            let cached = self.chk.as_mut().expect("SelectIntoExec must be opened");
            cached.Reset();
            self.source.Next(ctx.clone(), cached)?;
            if cached.NumRows() == 0 {
                break;
            }
            self.dumpToOutfile()?;
        }
        Ok(())
    }

    /// 可选包围模式下，字符串/时间/JSON 等类型需要包围。
    pub fn considerEncloseOpt(&self, eval_type: EvalType) -> bool {
        matches!(
            eval_type,
            ETString | ETDuration | ETTimestamp | ETDatetime | ETJson
        )
    }

    /// 按转义符处理 NUL、转义符本身、包围符及分隔符。
    pub fn escapeField(&mut self, field: &[u8]) -> &[u8] {
        let escaped_by = self.LineFieldsInfo.FieldsEscapedBy.as_bytes();
        if escaped_by.is_empty() {
            self.escapeBuf.clear();
            self.escapeBuf.extend_from_slice(field);
            return &self.escapeBuf;
        }
        let enclosed_by = self.LineFieldsInfo.FieldsEnclosedBy.as_bytes();
        let field_terminated_by = self.LineFieldsInfo.FieldsTerminatedBy.as_bytes();
        let line_terminated_by = self.LineFieldsInfo.LinesTerminatedBy.as_bytes();

        self.escapeBuf.clear();
        for &source_byte in field {
            let mut byte = source_byte;
            let escape = if byte == 0 {
                byte = b'0';
                true
            } else if byte == escaped_by[0]
                || enclosed_by
                    .first()
                    .is_some_and(|enclosed| byte == *enclosed)
            {
                true
            } else if !self.enclosed
                && field_terminated_by
                    .first()
                    .is_some_and(|terminator| byte == *terminator)
            {
                true
            } else {
                line_terminated_by
                    .first()
                    .is_some_and(|terminator| byte == *terminator)
            };
            if escape {
                self.escapeBuf.push(escaped_by[0]);
            }
            self.escapeBuf.push(byte);
        }
        &self.escapeBuf
    }

    /// 将缓存 chunk 中的行格式化后写入 outfile，并累计影响行数。
    pub fn dumpToOutfile(&mut self) -> Result<(), errors::SharedError> {
        let enclosed_by = self.LineFieldsInfo.FieldsEnclosedBy.as_bytes();
        let enclosure = enclosed_by.first().copied();
        let optionally_enclosed = self.LineFieldsInfo.FieldsOptEnclosed;
        let null_term = self
            .LineFieldsInfo
            .FieldsEscapedBy
            .as_bytes()
            .first()
            .map(|escaped| vec![*escaped, b'N'])
            .unwrap_or_else(|| b"NULL".to_vec());

        let row_count = self
            .chk
            .as_ref()
            .expect("SelectIntoExec must be opened")
            .NumRows();
        // 逐行逐列序列化：NULL、包围、按 MySQL 类型写出。
        for row_index in 0..row_count {
            let row = self
                .chk
                .as_ref()
                .expect("SelectIntoExec must be opened")
                .GetRow(row_index);
            self.lineBuf.clear();
            for column_index in 0..self.fieldTypes.len() {
                if column_index != 0 {
                    self.lineBuf
                        .extend_from_slice(self.LineFieldsInfo.FieldsTerminatedBy.as_bytes());
                }
                if row.IsNull(column_index) {
                    self.lineBuf.extend_from_slice(&null_term);
                    continue;
                }

                let field_type = self.fieldTypes[column_index].clone();
                let eval_type = field_type.EvalType();
                let should_enclose = enclosure.is_some()
                    && (!optionally_enclosed || self.considerEncloseOpt(eval_type));
                if should_enclose {
                    self.lineBuf.push(enclosure.unwrap());
                    self.enclosed = true;
                } else {
                    self.enclosed = false;
                }

                self.fieldBuf.clear();
                match field_type.GetType() {
                    mysql::TypeTiny
                    | mysql::TypeShort
                    | mysql::TypeInt24
                    | mysql::TypeLong
                    | mysql::TypeYear => {
                        self.fieldBuf
                            .extend_from_slice(row.GetInt64(column_index).to_string().as_bytes());
                    }
                    mysql::TypeLonglong => {
                        let value = if mysql::HasUnsignedFlag(field_type.GetFlag()) {
                            row.GetUint64(column_index).to_string()
                        } else {
                            row.GetInt64(column_index).to_string()
                        };
                        self.fieldBuf.extend_from_slice(value.as_bytes());
                    }
                    mysql::TypeFloat => {
                        (self.realBuf, self.fieldBuf) = DumpRealOutfile(
                            std::mem::take(&mut self.realBuf),
                            std::mem::take(&mut self.fieldBuf),
                            row.GetFloat32(column_index) as f64,
                            &field_type,
                        );
                    }
                    mysql::TypeDouble => {
                        (self.realBuf, self.fieldBuf) = DumpRealOutfile(
                            std::mem::take(&mut self.realBuf),
                            std::mem::take(&mut self.fieldBuf),
                            row.GetFloat64(column_index),
                            &field_type,
                        );
                    }
                    mysql::TypeNewDecimal => self
                        .fieldBuf
                        .extend_from_slice(&row.GetMyDecimal(column_index).ToString()),
                    mysql::TypeString
                    | mysql::TypeVarString
                    | mysql::TypeVarchar
                    | mysql::TypeTinyBlob
                    | mysql::TypeMediumBlob
                    | mysql::TypeLongBlob
                    | mysql::TypeBlob => {
                        self.fieldBuf.extend_from_slice(&row.GetBytes(column_index))
                    }
                    mysql::TypeBit => self.lineBuf.extend_from_slice(&row.GetBytes(column_index)),
                    mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => self
                        .fieldBuf
                        .extend_from_slice(row.GetTime(column_index).String().as_bytes()),
                    mysql::TypeDuration => self.fieldBuf.extend_from_slice(
                        row.GetDuration(column_index, field_type.GetDecimal() as i32)
                            .String()
                            .as_bytes(),
                    ),
                    mysql::TypeEnum => self
                        .fieldBuf
                        .extend_from_slice(row.GetEnum(column_index).Name.as_bytes()),
                    mysql::TypeSet => self
                        .fieldBuf
                        .extend_from_slice(row.GetSet(column_index).Name.as_bytes()),
                    mysql::TypeJSON => self
                        .fieldBuf
                        .extend_from_slice(row.GetJSON(column_index).String().as_bytes()),
                    mysql::TypeTiDBVectorFloat32 => self
                        .fieldBuf
                        .extend_from_slice(row.GetVectorFloat32(column_index).String().as_bytes()),
                    _ => {}
                }

                // 字符串与 JSON 需要转义；其余类型直接追加。
                if matches!(eval_type, ETString | ETJson) {
                    let field = self.fieldBuf.clone();
                    let escaped = self.escapeField(&field).to_vec();
                    self.lineBuf.extend_from_slice(&escaped);
                } else {
                    self.lineBuf.extend_from_slice(&self.fieldBuf);
                }
                if should_enclose {
                    self.lineBuf.push(enclosure.unwrap());
                }
            }
            self.lineBuf
                .extend_from_slice(self.LineFieldsInfo.LinesTerminatedBy.as_bytes());
            self.writer
                .as_mut()
                .expect("SelectIntoExec must be opened")
                .WriteAll(&self.lineBuf)
                .map_err(|error| errors::New(error.to_string()))?;
        }
        self.source.AddAffectedRows(row_count as u64);
        Ok(())
    }

    // Close always attempts all three phases and preserves Go's priority:
    // writer.Flush, destination Close, then child executor Close.
    /// 关闭：始终尝试 Flush、sink Close、子 Close，错误优先级与 Go 一致。
    pub fn Close(&mut self) -> Result<(), errors::SharedError> {
        if !self.started {
            return Ok(());
        }
        let flush_error = self.writer.as_mut().and_then(|writer| writer.Flush().err());
        let close_error = self.writer.as_mut().and_then(|writer| writer.Close().err());
        self.writer = None;
        let source_error = self.source.Close().err();
        self.started = false;

        if let Some(error) = flush_error {
            return Err(errors::New(error.to_string()));
        }
        if let Some(error) = close_error {
            return Err(errors::New(error.to_string()));
        }
        source_error.map_or(Ok(()), Err)
    }
}

// DumpRealOutfile dumps a real number to lineBuf.
/// 将浮点数按精度/量级格式化写入 `lineBuf`（极大极小用科学计数法）。
pub fn DumpRealOutfile(
    mut realBuf: Vec<u8>,
    mut lineBuf: Vec<u8>,
    value: f64,
    field_type: &types::FieldType,
) -> (Vec<u8>, Vec<u8>) {
    let precision =
        if field_type.GetDecimal() > 0 && field_type.GetDecimal() != mysql::NotFixedDec as isize {
            field_type.GetDecimal()
        } else {
            -1
        };
    let absolute = value.abs();
    // 默认精度且量级极端时用科学计数法，并去掉指数中的 '+'。
    if precision == -1
        && (absolute >= EXP_FORMAT_BIG || (absolute != 0.0 && absolute < EXP_FORMAT_SMALL))
    {
        realBuf.clear();
        if value.is_infinite() {
            realBuf.extend_from_slice(if value.is_sign_positive() {
                b"+Inf"
            } else {
                b"-Inf"
            });
        } else {
            realBuf.extend_from_slice(format!("{value:e}").as_bytes());
        }
        if let Some(plus) = realBuf.iter().position(|byte| *byte == b'+') {
            lineBuf.extend_from_slice(&realBuf[..plus]);
            lineBuf.extend_from_slice(&realBuf[plus + 1..]);
        } else {
            lineBuf.extend_from_slice(&realBuf);
        }
    } else if precision == -1 {
        if value.is_nan() {
            lineBuf.extend_from_slice(b"NaN");
        } else {
            lineBuf.extend_from_slice(value.to_string().as_bytes());
        }
    } else {
        if value.is_infinite() {
            lineBuf.extend_from_slice(if value.is_sign_positive() {
                b"+Inf"
            } else {
                b"-Inf"
            });
        } else if value.is_nan() {
            lineBuf.extend_from_slice(b"NaN");
        } else {
            lineBuf.extend_from_slice(
                format!("{value:.precision$}", precision = precision as usize).as_bytes(),
            );
        }
    }
    (realBuf, lineBuf)
}
