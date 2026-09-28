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

// mydump schema/SQL 语句读取与字符集解码。
//
// 从对象存储打开 dump 文件，按分号切分并过滤块注释，得到可执行 DDL/DML 文本；
// 支持 utf8mb4、gb18030、latin1 等编码转换。另提供带 WorkerPool 限流的
// `PooledReader`，在并发解析时限制同时进行的 I/O seek/read 次数。
use crate::{Compression, FileInfo, MydumpError};
use encoding_rs::{GB18030, WINDOWS_1252};
use std::io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, Condvar, Mutex};

/// 未找到 INSERT 语句时的错误文案（与 Go 常量对齐）。
pub const ErrInsertStatementNotFound: &str = "insert statement not found";
/// schema 文件编码非法时的错误文案。
const ERR_INVALID_SCHEMA_ENCODING: &str = "invalid schema encoding";

/// 按字符集名将原始字节解码为 UTF-8 字节；`binary` 原样返回。
pub fn decodeCharacterSet(data: Vec<u8>, charset: &str) -> Result<Vec<u8>, MydumpError> {
    match charset {
        "binary" => Ok(data),
        // auto/utf8mb4：若已是合法 UTF-8 则直接通过。
        "auto" | "utf8mb4" if std::str::from_utf8(&data).is_ok() => Ok(data),
        "utf8mb4" => Err(MydumpError::Encoding(ERR_INVALID_SCHEMA_ENCODING.into())),
        "auto" | "gb18030" => {
            let (decoded, _, bad) = GB18030.decode(&data);
            if bad || decoded.contains('\u{fffd}') {
                Err(MydumpError::Encoding(ERR_INVALID_SCHEMA_ENCODING.into()))
            } else {
                Ok(decoded.into_owned().into_bytes())
            }
        }
        "latin1" => {
            let (decoded, _, bad) = WINDOWS_1252.decode(&data);
            if bad || decoded.contains('\u{fffd}') {
                Err(MydumpError::Encoding(ERR_INVALID_SCHEMA_ENCODING.into()))
            } else {
                Ok(decoded.into_owned().into_bytes())
            }
        }
        other => Err(MydumpError::Encoding(format!(
            "Unsupported encoding {other}"
        ))),
    }
}
/// snake_case 别名，转发到 `decodeCharacterSet`。
pub fn decode_character_set(data: Vec<u8>, charset: &str) -> Result<Vec<u8>, MydumpError> {
    decodeCharacterSet(data, charset)
}

/// 对象存储抽象：按路径打开可读流，并可列举文件。
pub trait Storage: Send + Sync {
    /// 打开指定路径；`compression` 指示外层压缩格式（如 gz/zstd）。
    fn open(
        &self,
        path: &str,
        compression: Compression,
    ) -> Result<Box<dyn Read + Send>, MydumpError>;
    /// 列举存储中的路径与大小；默认返回空列表。
    fn list(&self) -> Result<Vec<(String, i64)>, MydumpError> {
        Ok(Vec::new())
    }
}
/// 从 schema 文件导出完整 SQL 语句字节（去掉块注释，校验末尾分号后解码）。
pub fn ExportStatement(
    store: &dyn Storage,
    file: &FileInfo,
    charset: &str,
) -> Result<Vec<u8>, MydumpError> {
    let mut reader = BufReader::new(store.open(&file.file_meta.path, file.file_meta.compression)?);
    let mut data = Vec::new();
    let mut stmt = Vec::new();
    let mut line = Vec::new();
    let mut first = true;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if first {
            first = false;
            // 去掉 UTF-8 BOM，避免污染首条语句。
            if line.starts_with(&[0xef, 0xbb, 0xbf]) {
                line.drain(..3);
            }
        }
        let line = trim_ascii(&line);
        if line.is_empty() {
            continue;
        }
        stmt.extend_from_slice(line);
        // 以分号结束则提交一条语句；纯块注释 `/*...*/;` 丢弃。
        if stmt.last() == Some(&b';') {
            if !(stmt.starts_with(b"/*") && stmt.ends_with(b"*/;")) {
                data.extend_from_slice(&stmt)
            }
            stmt.clear()
        } else {
            stmt.push(b'\n')
        }
    }
    let rest = trim_ascii(&stmt);
    // 文件末尾若仍有非注释内容且无分号，视为语法错误。
    if !rest.is_empty() && !(rest.starts_with(b"/*") && rest.ends_with(b"*/")) {
        return Err(MydumpError::Syntax(format!(
            "last SQL statement missing trailing semicolon; file: {}",
            file.file_meta.path
        )));
    }
    decodeCharacterSet(data, charset)
}
/// snake_case 别名，转发到 `ExportStatement`。
pub fn export_statement(
    store: &dyn Storage,
    file: &FileInfo,
    charset: &str,
) -> Result<Vec<u8>, MydumpError> {
    ExportStatement(store, file, charset)
}
/// 去掉 ASCII 空白首尾，返回子切片。
fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..]
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1]
    }
    value
}

/// 可读、可 seek、可跨线程发送的流接口。
pub trait ReadSeekCloser: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeekCloser for T {}
/// 基于内存游标的字符串/字节读取器。
pub struct StringReader(Cursor<Vec<u8>>);
impl StringReader {
    /// 由字节向量构造。
    pub fn from_bytes(value: Vec<u8>) -> Self {
        Self(Cursor::new(value))
    }
}
/// 由字符串构造 `StringReader`（对应 Go NewStringReader）。
pub fn NewStringReader(value: &str) -> StringReader {
    StringReader(Cursor::new(value.as_bytes().to_vec()))
}
impl Read for StringReader {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl Seek for StringReader {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(p)
    }
}
impl StringReader {
    /// 关闭读取器（内存实现为空操作）。
    pub fn Close(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[derive(Debug)]
/// 限制并发 I/O 的工作池：最多 `limit` 个令牌。
pub struct WorkerPool {
    /// 允许同时占用的最大令牌数。
    limit: usize,
    /// 当前已占用令牌数。
    used: Mutex<usize>,
    /// 有令牌释放时唤醒等待者。
    ready: Condvar,
}
impl WorkerPool {
    /// 创建工作池；`limit` 至少为 1。
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit: limit.max(1),
            used: Mutex::new(0),
            ready: Condvar::new(),
        })
    }
    /// 阻塞直到取得一个令牌，返回在 Drop 时自动释放的守卫。
    fn acquire(self: &Arc<Self>) -> WorkerGuard {
        let mut used = self.used.lock().unwrap();
        while *used >= self.limit {
            used = self.ready.wait(used).unwrap()
        }
        *used += 1;
        WorkerGuard(Arc::clone(self))
    }
}
/// RAII 令牌守卫：离开作用域时归还令牌并通知等待者。
struct WorkerGuard(Arc<WorkerPool>);
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        *self.0.used.lock().unwrap() -= 1;
        self.0.ready.notify_one()
    }
}
/// 包装底层 `ReadSeekCloser`，在 read/seek 时可选地占用 WorkerPool 令牌。
pub struct PooledReader {
    /// 底层可读可 seek 流。
    reader: Option<Box<dyn ReadSeekCloser>>,
    /// 可选的并发限流池；为 None 时不做限流。
    workers: Option<Arc<WorkerPool>>,
}
/// 构造带可选工作池的 `PooledReader`。
pub fn MakePooledReader(
    reader: Box<dyn ReadSeekCloser>,
    workers: Option<Arc<WorkerPool>>,
) -> PooledReader {
    PooledReader {
        reader: Some(reader),
        workers,
    }
}
impl Read for PooledReader {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let _g = self.workers.as_ref().map(WorkerPool::acquire);
        self.reader.as_mut().ok_or_else(reader_closed)?.read(b)
    }
}
impl Seek for PooledReader {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        // 仅查询当前位置（Current(0)）不占用令牌，避免无谓限流。
        if matches!(p, SeekFrom::Current(0)) {
            return self.reader.as_mut().ok_or_else(reader_closed)?.seek(p);
        }
        let _g = self.workers.as_ref().map(WorkerPool::acquire);
        self.reader.as_mut().ok_or_else(reader_closed)?.seek(p)
    }
}
impl PooledReader {
    /// 关闭读取器并立即释放底层资源；重复关闭为空操作。
    pub fn Close(&mut self) -> std::io::Result<()> {
        self.reader.take();
        Ok(())
    }
    /// 读满缓冲区；失败时返回底层错误（对应 Go ReadFull）。
    pub fn ReadFull(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let _g = self.workers.as_ref().map(WorkerPool::acquire);
        self.reader
            .as_mut()
            .ok_or_else(reader_closed)?
            .read_exact(b)
            .map(|_| b.len())
    }
}

fn reader_closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "reader is closed")
}
impl PooledReader {
    /// PascalCase 别名，转发到 `Read`。
    pub fn Read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.read(b)
    }
    /// PascalCase 别名，转发到 `Seek`。
    pub fn Seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.seek(p)
    }
}
