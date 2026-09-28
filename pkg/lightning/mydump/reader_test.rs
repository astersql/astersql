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

// `ExportStatement` 与字符集/压缩读取的单元测试。
//
// 覆盖无尾换行、块注释过滤、GBK 解码、乱码错误、非 EOF I/O 错误、
// gzip 压缩 schema，以及缺失末尾分号与 UTF-8 BOM 等边界情况。
use crate::test_support::{MemoryStorage, file};
use crate::{
    Compression, ExportStatement, MakePooledReader, MydumpError, SourceType, Storage,
    decodeCharacterSet,
};
use std::io::{Error, ErrorKind, Read, Seek, SeekFrom};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// 将 input 写入内存存储后导出，并断言结果等于 expected。
fn exportStatmentShouldBe(input: &[u8], expected: &[u8]) {
    let store = MemoryStorage::with(&[("schema.sql", input)]);
    let info = file("schema.sql", SourceType::TableSchema);
    assert_eq!(ExportStatement(&store, &info, "auto").unwrap(), expected);
}

#[test]
/// 无尾换行的单条 CREATE DATABASE 语句应原样导出。
fn TestExportStatementNoTrailingNewLine() {
    exportStatmentShouldBe(b"CREATE DATABASE whatever;", b"CREATE DATABASE whatever;");
}

#[test]
/// 多行块注释后的 CREATE 语句应丢弃注释、保留 DDL。
fn TestExportStatementWithComment() {
    exportStatmentShouldBe(
        b"/* whatever\n multiple lines comment\n */;\nCREATE DATABASE whatever;\n",
        b"CREATE DATABASE whatever;",
    );
}

#[test]
/// 块注释 + CREATE 且文件无尾换行时仍应正确导出。
fn TestExportStatementWithCommentNoTrailingNewLine() {
    exportStatmentShouldBe(
        b"/* whatever\n multiple lines comment\n */;\nCREATE DATABASE whatever;",
        b"CREATE DATABASE whatever;",
    );
}

#[test]
/// GBK/GB18030 注释内容在 auto 模式下应解码为 UTF-8。
fn TestExportStatementGBK() {
    let mut input = b"CREATE TABLE a (b int(11) COMMENT '".to_vec();
    input.extend_from_slice(&[0xD7, 0xDC, 0xB0, 0xB8, 0xC0, 0xFD]);
    input.extend_from_slice(b"');\n");
    let store = MemoryStorage::default();
    store.insert("schema.sql", input);
    let result =
        ExportStatement(&store, &file("schema.sql", SourceType::TableSchema), "auto").unwrap();
    assert_eq!(
        result,
        "CREATE TABLE a (b int(11) COMMENT '总案例');".as_bytes()
    );
}

#[test]
/// 无法解码的乱码字节应返回编码错误。
fn TestExportStatementGibberishError() {
    let input = b"\x9e\x02\xdc\xfbZ/=n\xf3\xf2N8\xc1\xf2\xe9\xaa\xd0\x85\xc5}\x97\x07\xae6\x97\x99\x9c\x08\xcb\xe8;";
    let store = MemoryStorage::with(&[("schema.sql", input)]);
    assert!(ExportStatement(&store, &file("schema.sql", SourceType::TableSchema), "auto").is_err());
}

#[test]
/// 字符集名称与 Go switch 一样区分大小写，不能静默接受未知别名。
fn TestDecodeCharacterSetRejectsMismatchedCase() {
    let error = decodeCharacterSet(b"SELECT 1;".to_vec(), "UTF8MB4").unwrap_err();
    assert_eq!(
        error.to_string(),
        "encoding error: Unsupported encoding UTF8MB4"
    );
}

struct DropTrackingReader {
    dropped: Arc<AtomicBool>,
}

impl Read for DropTrackingReader {
    fn read(&mut self, _data: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}

impl Seek for DropTrackingReader {
    fn seek(&mut self, _position: SeekFrom) -> std::io::Result<u64> {
        Ok(0)
    }
}

impl Drop for DropTrackingReader {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[test]
/// Close 必须立即释放底层 reader；关闭后不可继续执行 I/O。
fn TestPooledReaderCloseReleasesUnderlyingReader() {
    let dropped = Arc::new(AtomicBool::new(false));
    let mut reader = MakePooledReader(
        Box::new(DropTrackingReader {
            dropped: Arc::clone(&dropped),
        }),
        None,
    );

    reader.Close().unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(
        reader.Read(&mut [0]).unwrap_err().kind(),
        ErrorKind::BrokenPipe
    );
}

/// 始终返回 PermissionDenied 的假读取器，用于错误路径测试。
struct AlwaysErrorReadSeekCloser;

impl AlwaysErrorReadSeekCloser {
    fn Read(&mut self, _data: &mut [u8]) -> std::io::Result<usize> {
        Err(Error::new(ErrorKind::PermissionDenied, "read error"))
    }
    fn Seek(&mut self, _position: SeekFrom) -> std::io::Result<u64> {
        Err(Error::new(ErrorKind::PermissionDenied, "seek error"))
    }
    fn Close(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn GetFileSize(&self) -> std::io::Result<i64> {
        Err(Error::new(
            ErrorKind::PermissionDenied,
            "get file size error",
        ))
    }
}

impl Read for AlwaysErrorReadSeekCloser {
    fn read(&mut self, data: &mut [u8]) -> std::io::Result<usize> {
        self.Read(data)
    }
}

impl Seek for AlwaysErrorReadSeekCloser {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.Seek(position)
    }
}

/// 打开时返回 AlwaysErrorReadSeekCloser 的假存储。
struct ErrorStorage;
impl Storage for ErrorStorage {
    fn open(
        &self,
        _path: &str,
        _compression: Compression,
    ) -> Result<Box<dyn Read + Send>, MydumpError> {
        Ok(Box::new(AlwaysErrorReadSeekCloser))
    }
}

#[test]
/// 非 EOF 读错误应向上传播，Seek/GetFileSize 同样失败。
fn TestExportStatementHandleNonEOFError() {
    let error = ExportStatement(
        &ErrorStorage,
        &file("no-perm-file", SourceType::TableSchema),
        "auto",
    )
    .unwrap_err();
    assert!(error.to_string().contains("read error"));

    let mut reader = AlwaysErrorReadSeekCloser;
    assert!(reader.Seek(SeekFrom::Start(0)).is_err());
    assert!(reader.GetFileSize().is_err());
    reader.Close().unwrap();
}

#[test]
/// 带 Gz 压缩元数据的 schema 文件应能导出语句。
fn TestExportStatementCompressed() {
    let store = MemoryStorage::with(&[("schema.sql.gz", b"CREATE DATABASE whatever;")]);
    let mut info = file("schema.sql.gz", SourceType::SchemaSchema);
    info.file_meta.compression = Compression::Gz;
    assert_eq!(
        ExportStatement(&store, &info, "auto").unwrap(),
        b"CREATE DATABASE whatever;"
    );
}

#[test]
/// 表驱动：缺分号报错；纯注释/BOM/尾注释等合法边界应通过。
fn TestExportStatementMissingTrailingSemicolon() {
    let cases: &[(&[u8], Option<&[u8]>)] = &[
        (b"CREATE DATABASE whatever", None),
        (b"/* only comment */\n", Some(b"")),
        (
            b"CREATE DATABASE whatever;\n/* trailing comment */\n",
            Some(b"CREATE DATABASE whatever;"),
        ),
        (b"CREATE DATABASE whatever;\n-- trailing comment", None),
        (
            b"\xef\xbb\xbfCREATE DATABASE whatever;",
            Some(b"CREATE DATABASE whatever;"),
        ),
    ];
    for (index, (input, expected)) in cases.iter().enumerate() {
        let path = format!("schema-{index}.sql");
        let store = MemoryStorage::default();
        store.insert(&path, input.to_vec());
        let result = ExportStatement(&store, &file(&path, SourceType::TableSchema), "auto");
        match expected {
            Some(expected) => assert_eq!(result.unwrap(), *expected),
            None => assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("missing trailing semicolon")
            ),
        }
    }
}
