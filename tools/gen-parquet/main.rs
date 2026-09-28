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

//! 生成与 `main.go` 相同的双列 Parquet 测试夹具。
//!
//! 每个分片只包含一个行组，列值分别为递增整数及其十进制字符串形式；
//! 命令行参数、文件命名和资源关闭顺序均保持 Go 版本的行为。

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parquet::basic::{Compression, Repetition, Type as PhysicalType};
use parquet::column::writer::ColumnWriter;
use parquet::data_type::ByteArray;
use parquet::file::properties::WriterProperties;
use parquet::file::writer::{SerializedFileWriter, SerializedRowGroupWriter};
use parquet::schema::types::{ColumnPath, Type};

/// 迁移期间供独立工具 crate 使用的兼容桩。
pub mod stubs;

/// 对应 Go 的 `writeWrapper`，为 Parquet 写入器封装目标文件。
///
/// Parquet 写入流程实际只依赖 `Write`；`Read` 和 `Seek` 的空实现仅用于满足
/// 兼容接口，不能据此读取文件或定位写入位置。
pub struct WriteWrapper {
    /// 接收最终 Parquet 字节的文件。
    pub writer: File,
}

impl Seek for WriteWrapper {
    fn seek(&mut self, _pos: SeekFrom) -> io::Result<u64> {
        Ok(0)
    }
}

impl Read for WriteWrapper {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}

impl Write for WriteWrapper {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writer.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl WriteWrapper {
    /// 刷新尚未落盘的数据；文件句柄随后随包装器析构而关闭。
    pub fn close(mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// 构造与 Go `getParquetWriter` 等价的写入器。
///
/// 所有列均为可选列，并逐列启用字典编码和 Snappy 压缩；列名与物理类型
/// 必须一一对应，避免静默截断 `zip` 后的 schema。
pub fn get_parquet_writer<W: Write + Send>(
    writer: W,
    row_names: &[String],
    row_types: &[PhysicalType],
) -> Result<SerializedFileWriter<W>, String> {
    if row_names.len() != row_types.len() {
        return Err("row names and row types must have equal lengths".to_string());
    }

    let mut fields = Vec::with_capacity(row_names.len());
    let mut properties = WriterProperties::builder();
    for (name, physical_type) in row_names.iter().zip(row_types) {
        let field = Type::primitive_type_builder(name, *physical_type)
            .with_repetition(Repetition::OPTIONAL)
            .with_id(Some(8))
            .build()
            .map_err(|err| err.to_string())?;
        fields.push(Arc::new(field));

        let path = ColumnPath::from(name.clone());
        properties = properties
            .set_column_dictionary_enabled(path.clone(), true)
            .set_column_compression(path, Compression::SNAPPY);
    }

    let root = Type::group_type_builder("schema")
        .with_repetition(Repetition::REQUIRED)
        .with_fields(fields)
        .build()
        .map(Arc::new)
        .map_err(|err| err.to_string())?;
    SerializedFileWriter::new(writer, root, Arc::new(properties.build()))
        .map_err(|err| err.to_string())
}

/// 向行组的下一个物理列写入 `0..rows`，对应 Go 的 `writeColumn`。
///
/// 定义级别全部为 1，表示可选列中的每个值都存在；具体值会按物理类型转换为
/// 整数、浮点数或十进制字符串，其他列类型则显式报错。
pub fn write_column<W: Write + Send>(
    row_group: &mut SerializedRowGroupWriter<'_, W>,
    rows: usize,
) -> Result<(), String> {
    let mut column = row_group
        .next_column()
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "no more columns".to_string())?;
    let definition_levels = vec![1_i16; rows];

    match column.untyped() {
        ColumnWriter::Int64ColumnWriter(writer) => {
            let values: Vec<i64> = (0..rows).map(|value| value as i64).collect();
            writer
                .write_batch(&values, Some(&definition_levels), None)
                .map_err(|err| err.to_string())?;
        }
        ColumnWriter::DoubleColumnWriter(writer) => {
            let values: Vec<f64> = (0..rows).map(|value| value as f64).collect();
            writer
                .write_batch(&values, Some(&definition_levels), None)
                .map_err(|err| err.to_string())?;
        }
        ColumnWriter::ByteArrayColumnWriter(writer) => {
            let values: Vec<ByteArray> = (0..rows)
                .map(|value| ByteArray::from(value.to_string().as_str()))
                .collect();
            writer
                .write_batch(&values, Some(&definition_levels), None)
                .map_err(|err| err.to_string())?;
        }
        _ => return Err("unsupported column type".to_string()),
    }

    // 对齐 Go 的 defer：关闭错误刻意忽略，不能覆盖前面的批量写入结果。
    let _ = column.close();
    Ok(())
}

/// 生成一个包含单行组、`iVal` 与 `s` 两列的 Parquet 分片。
pub fn write_simple_parquet_file(file_path: &str, rows: i32) -> Result<(), String> {
    let rows = usize::try_from(rows).map_err(|_| format!("negative row count: {rows}"))?;
    let file = File::create(file_path).map_err(|err| err.to_string())?;
    let wrapper = WriteWrapper { writer: file };
    let row_names = vec!["iVal".to_string(), "s".to_string()];
    let row_types = vec![PhysicalType::INT64, PhysicalType::BYTE_ARRAY];
    let mut parquet_writer = get_parquet_writer(wrapper, &row_names, &row_types)?;

    let mut row_group = parquet_writer
        .next_row_group()
        .map_err(|err| err.to_string())?;
    let mut write_error = None;
    for _ in &row_names {
        if let Err(err) = write_column(&mut row_group, rows) {
            write_error = Some(err);
            break;
        }
    }
    // 两次关闭对应 Go 中忽略结果的 defer：写入失败时也必须执行，且不能覆盖
    // WriteBatch 返回的原始错误。
    let _ = row_group.close();
    let _ = parquet_writer.close();
    write_error.map_or(Ok(()), Err)
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 生成命令的参数；默认值与 Go 版本注册的 flag 保持一致。
pub struct Flags {
    /// 输出文件名中的 schema 部分。
    pub schema_name: String,
    /// 输出文件名中的表名部分。
    pub table_name: String,
    /// 要生成的分片文件数量。
    pub chunks: i32,
    /// 每个分片的行数。
    pub row_numbers: i32,
    /// 输出目录；空字符串表示当前目录。
    pub source_dir: String,
}

impl Default for Flags {
    fn default() -> Self {
        Self {
            schema_name: "test".into(),
            table_name: "parquet".into(),
            chunks: 10,
            row_numbers: 1000,
            source_dir: String::new(),
        }
    }
}

/// 解析本命令使用的 Go `flag` 形式，兼容单横线、双横线及 `key=value`。
///
/// 与 Go `flag.Parse` 一样，遇到 `--` 或首个位置参数即停止解析。
pub fn parse_flags(args: &[String]) -> Result<Flags, String> {
    let mut flags = Flags::default();
    let mut index = 1;
    while index < args.len() {
        let argument = &args[index];
        if argument == "--" {
            break;
        }
        if argument == "-" || !argument.starts_with('-') {
            break;
        }
        let rest = argument
            .strip_prefix("--")
            .or_else(|| argument.strip_prefix('-'))
            .expect("dash prefix checked above");
        let (key, inline_value) = split_flag(rest);
        if !matches!(key, "schema" | "table" | "chunk" | "rows" | "dir") {
            return Err(format!("flag provided but not defined: -{key}"));
        }
        let value = match inline_value {
            Some(value) => value.to_string(),
            None => {
                index += 1;
                args.get(index)
                    .cloned()
                    .ok_or_else(|| format!("flag needs an argument: -{key}"))?
            }
        };
        match key {
            "schema" => flags.schema_name = value,
            "table" => flags.table_name = value,
            "chunk" => {
                flags.chunks = value
                    .parse()
                    .map_err(|_| format!("invalid value {value:?} for flag -chunk"))?
            }
            "rows" => {
                flags.row_numbers = value
                    .parse()
                    .map_err(|_| format!("invalid value {value:?} for flag -rows"))?
            }
            "dir" => flags.source_dir = value,
            _ => unreachable!(),
        }
        index += 1;
    }
    Ok(flags)
}

fn split_flag(rest: &str) -> (&str, Option<&str>) {
    rest.split_once('=')
        .map_or((rest, None), |(key, value)| (key, Some(value)))
}

/// 按 `<schema>.<table>.<四位序号>.parquet` 生成分片文件名。
pub fn chunk_file_name(schema: &str, table: &str, index: i32) -> String {
    format!("{schema}.{table}.{index:04}.parquet")
}

/// 将分片文件名拼接到输出目录，保留平台原生路径规则。
pub fn chunk_file_path(dir: &str, schema: &str, table: &str, index: i32) -> PathBuf {
    Path::new(dir).join(chunk_file_name(schema, table, index))
}

/// 依次生成全部分片，并在错误中补充失败的文件名。
pub fn run(flags: &Flags) -> Result<(), String> {
    for index in 0..flags.chunks {
        let name = chunk_file_name(&flags.schema_name, &flags.table_name, index);
        let path = chunk_file_path(
            &flags.source_dir,
            &flags.schema_name,
            &flags.table_name,
            index,
        );
        write_simple_parquet_file(&path.to_string_lossy(), flags.row_numbers)
            .map_err(|err| format!("generate test source failed, name: {name}, err: {err}"))?;
    }
    Ok(())
}

/// 解析进程参数并执行生成任务，按参数错误和运行错误返回不同退出码。
pub fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flags = match parse_flags(&args) {
        Ok(flags) => flags,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    if let Err(err) = run(&flags) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
