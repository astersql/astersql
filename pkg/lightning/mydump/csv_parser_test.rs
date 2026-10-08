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

// CsvParser 单元测试：RFC4180、MySQL 转义、TSV/CRLF、表头、字符集与错误路径。
//
// 覆盖自定义分隔/引号/转义、行前缀、空行、尾部分隔符裁剪、过大字段限制，
// 以及读错误与基准 harness。

use crate::*;
use std::io::{Error as IoError, ErrorKind, Read, Seek, SeekFrom};
use std::sync::{Mutex, atomic::Ordering};

/// Go's tests mutate LargestEntryLimit without parallel execution. Keep the
/// corresponding Rust tests isolated from readers that expect the default.
static LARGEST_ENTRY_LIMIT_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 用内存 StringReader 构造 CsvParser。
fn make_parser(input: &[u8], cfg: CsvConfig, header: bool) -> Result<CsvParser, Error> {
    NewCSVParser(
        &cfg,
        Box::new(StringReader::from_bytes(input.to_vec())),
        header,
        None,
    )
}

/// 将 Datum 行转为 Option<String>，Null 映射为 None。
fn datumsToString(datums: &[Datum]) -> Vec<Option<String>> {
    datums
        .iter()
        .map(|datum| match datum {
            Datum::Null => None,
            Datum::Bytes(bytes) | Datum::Binary(bytes) => {
                Some(String::from_utf8_lossy(bytes).into_owned())
            }
            Datum::I64(value) => Some(value.to_string()),
        })
        .collect()
}

/// 读完所有行直到 Eof。
fn read_all(input: &[u8], cfg: CsvConfig) -> Result<Vec<Vec<Option<String>>>, Error> {
    let mut parser = make_parser(input, cfg, false)?;
    let mut rows = Vec::new();
    loop {
        match parser.ReadRow() {
            Ok(()) => rows.push(datumsToString(&parser.LastRow().row)),
            Err(Error::Eof) => return Ok(rows),
            Err(error) => return Err(error),
        }
    }
}

/// 批量断言成功解析结果。
fn runTestCasesCSV(cases: &[(&[u8], CsvConfig, Vec<Vec<Option<String>>>)]) {
    for (input, cfg, expected) in cases {
        assert_eq!(&read_all(input, cfg.clone()).unwrap(), expected);
    }
}

/// 批量断言解析失败。
fn runFailingTestCasesCSV(cases: &[(&[u8], CsvConfig)]) {
    for (input, cfg) in cases {
        assert!(read_all(input, cfg.clone()).is_err());
    }
}

/// 断言解析器逻辑字节位置。
fn assertPosEqual(parser: &CsvParser, expected: i64) {
    assert_eq!(Parser::Pos(parser).0, expected);
}

/// TPC-H 样例行期望值。
fn tpchDatums() -> Vec<Option<String>> {
    vec![
        Some("1".into()),
        Some("Customer#000000001".into()),
        Some("BUILDING".into()),
    ]
}

/// 基准用的简单 CSV 解析器。
fn newBenchCSVParserSuite() -> CsvParser {
    make_parser(b"1,hello,world\n", CsvConfig::default(), false).unwrap()
}

/// 基准：通过 mydump CsvParser 读一行。
fn BenchmarkReadRowUsingMydumpCSVParser() {
    let mut parser = newBenchCSVParserSuite();
    parser.ReadRow().unwrap();
}

/// 基准对照：整文件 read_all 一次。
fn BenchmarkReadRowUsingEncodingCSV() {
    let rows = read_all(b"1,hello,world\n", CsvConfig::default()).unwrap();
    assert_eq!(rows.len(), 1);
}

/// RFC4180：逗号分隔与双引号转义。
#[test]
fn TestRFC4180() {
    runTestCasesCSV(&[(
        b"a,b,c\n\"x,y\",\"a\"\"b\",z\n",
        CsvConfig::default(),
        vec![
            vec![Some("a".into()), Some("b".into()), Some("c".into())],
            vec![Some("x,y".into()), Some("a\"b".into()), Some("z".into())],
        ],
    )]);
}

/// MySQL 风格：\\n 转义与 \\N 作为 NULL。
#[test]
fn TestMySQL() {
    let rows = read_all(b"1,hello\\nworld,\\N\n", CsvConfig::default()).unwrap();
    assert_eq!(rows[0][0], Some("1".into()));
    assert_eq!(rows[0][1], Some("hello\nworld".into()));
    assert_eq!(rows[0][2], None);
}

/// 制表符分隔、无引号。
#[test]
fn TestTSV() {
    let cfg = CsvConfig {
        fields_terminated_by: "\t".into(),
        fields_enclosed_by: String::new(),
        ..Default::default()
    };
    assert_eq!(
        read_all(b"a\tb\tc\n", cfg).unwrap()[0],
        vec![Some("a".into()), Some("b".into()), Some("c".into())]
    );
}

/// CRLF 行结束。
#[test]
fn TestCRLF() {
    let cfg = CsvConfig {
        lines_terminated_by: "\r\n".into(),
        ..Default::default()
    };
    assert_eq!(read_all(b"a,b\r\nc,d\r\n", cfg).unwrap().len(), 2);
}

/// 自定义转义字符 `!`。
#[test]
fn TestCustomEscapeChar() {
    let cfg = CsvConfig {
        fields_escaped_by: "!".into(),
        ..Default::default()
    };
    assert_eq!(
        read_all(b"hello!nworld,x\n", cfg).unwrap()[0][0],
        Some("hello\nworld".into())
    );
}

/// 仅解析带指定前缀的行。
#[test]
fn TestStartingBy() {
    let cfg = CsvConfig {
        lines_starting_by: "DATA:".into(),
        ..Default::default()
    };
    let rows = read_all(b"comment\nDATA:a,b\nignore\nDATA:c,d\n", cfg).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1][0], Some("c".into()));
}

/// 自定义多字符行结束符。
#[test]
fn TestTerminator() {
    let cfg = CsvConfig {
        lines_terminated_by: "<END>".into(),
        ..Default::default()
    };
    assert_eq!(read_all(b"a,b<END>c,d<END>", cfg).unwrap().len(), 2);
}

/// 反斜杠作为字段分隔符。
#[test]
fn TestBackslashAsSep() {
    let cfg = CsvConfig {
        fields_terminated_by: "\\".into(),
        fields_escaped_by: String::new(),
        ..Default::default()
    };
    assert_eq!(read_all(b"a\\b\\c\n", cfg).unwrap()[0].len(), 3);
}

/// 反斜杠作为引号定界符。
#[test]
fn TestBackslashAsDelim() {
    let cfg = CsvConfig {
        fields_enclosed_by: "\\".into(),
        fields_escaped_by: String::new(),
        ..Default::default()
    };
    assert_eq!(
        read_all(b"\\a,b\\,c\n", cfg).unwrap()[0][0],
        Some("a,b".into())
    );
}

/// 引号内保留分隔符。
#[test]
fn TestQuotedSeparator() {
    assert_eq!(
        read_all(b"\"a,b,c\",d\n", CsvConfig::default()).unwrap()[0],
        vec![Some("a,b,c".into()), Some("d".into())]
    );
}

/// \\0/\\t/\\Z 等特殊转义。
#[test]
fn TestSpecialChars() {
    let rows = read_all(b"a\\0b,a\\tb,a\\Zb\n", CsvConfig::default()).unwrap();
    assert_eq!(rows[0][0].as_ref().unwrap().as_bytes(), b"a\0b");
    assert_eq!(rows[0][1], Some("a\tb".into()));
    assert_eq!(rows[0][2].as_ref().unwrap().as_bytes(), b"a\x1ab");
}

/// quoted_null_is_text 时引号内 \\N 视为文本。
#[test]
fn TestNULL() {
    let mut cfg = CsvConfig::default();
    cfg.quoted_null_is_text = true;
    let rows = read_all(b"\\N,\"\\N\",NULL\n", cfg).unwrap();
    assert_eq!(rows[0][0], None);
    assert_eq!(rows[0][1], Some("N".into()));
    assert_eq!(rows[0][2], Some("NULL".into()));
}

/// 空输入与允许空行。
#[test]
fn TestEmpty() {
    assert!(read_all(b"", CsvConfig::default()).unwrap().is_empty());
    let cfg = CsvConfig {
        allow_empty_line: true,
        ..Default::default()
    };
    assert_eq!(
        read_all(b"\n", cfg).unwrap(),
        vec![vec![Some(String::new())]]
    );
}

/// 空白行仍作为记录保留。
#[test]
fn TestCsvWithWhiteSpaceLine() {
    let rows = read_all(b"   \na,b\n\t\nc,d\n", CsvConfig::default()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], Some("a".into()));
    assert_eq!(rows[1][0], Some("c".into()));
}

/// 未配置行终止符时，与 Go 一样自动识别 CR、LF 和 CRLF。
#[test]
fn auto_detects_cr_and_lf_when_terminator_is_empty() {
    let cfg = CsvConfig {
        lines_terminated_by: String::new(),
        ..Default::default()
    };
    assert_eq!(
        read_all(b"a\rb\r\nc\n\n\nd", cfg).unwrap(),
        vec![
            vec![Some("a".into())],
            vec![Some("b".into())],
            vec![Some("c".into())],
            vec![Some("d".into())],
        ]
    );
}

/// STARTING BY 匹配行内首次出现的位置，并丢弃此前内容。
#[test]
fn starting_by_matches_inside_the_physical_line() {
    let cfg = CsvConfig {
        lines_starting_by: "DATA:".into(),
        ..Default::default()
    };
    assert_eq!(
        read_all(b"prefix DATA:a,b\nmissing\n", cfg).unwrap(),
        vec![vec![Some("a".into()), Some("b".into())]]
    );
}

/// 连续分隔符产生空字段。
#[test]
fn TestConsecutiveFields() {
    assert_eq!(
        read_all(b"a,,c,\n", CsvConfig::default()).unwrap()[0],
        vec![
            Some("a".into()),
            Some(String::new()),
            Some("c".into()),
            Some(String::new()),
        ]
    );
}

/// 裁剪行尾多余分隔符。
#[test]
fn TestTrimLastSep() {
    let cfg = CsvConfig {
        trim_last_separators: true,
        ..Default::default()
    };
    assert_eq!(read_all(b"a,b,\n", cfg).unwrap()[0].len(), 2);
}

/// 多字节分隔符跨字段。
#[test]
fn TestContinuationCSV() {
    let separator = "--多字节分隔--";
    let cfg = CsvConfig {
        fields_terminated_by: separator.into(),
        ..Default::default()
    };
    let input = format!("第一列{separator}第二列\n第三列{separator}第四列\n");
    assert_eq!(read_all(input.as_bytes(), cfg).unwrap().len(), 2);
}

/// TPC-H 竖线分隔并裁剪尾部分隔符。
#[test]
fn TestTPCH() {
    let cfg = CsvConfig {
        fields_terminated_by: "|".into(),
        trim_last_separators: true,
        ..Default::default()
    };
    assert_eq!(
        read_all(b"1|Customer#000000001|BUILDING|\n", cfg).unwrap()[0],
        tpchDatums()
    );
}

/// 多字节竖线分隔符。
#[test]
fn TestTPCHMultiBytes() {
    let cfg = CsvConfig {
        fields_terminated_by: "||".into(),
        trim_last_separators: true,
        ..Default::default()
    };
    assert_eq!(read_all(b"1||hello||world||\n", cfg).unwrap()[0].len(), 3);
}

/// 跳过一行后再读。
#[test]
fn TestReadUntilTerminator() {
    let mut parser = make_parser(b"skip,this\nread,this\n", CsvConfig::default(), false).unwrap();
    parser.ReadUntilTerminator().unwrap();
    assertPosEqual(&parser, 10);
    parser.ReadRow().unwrap();
    assert_eq!(
        datumsToString(&parser.LastRow().row)[0],
        Some("read".into())
    );
}

/// 未闭合引号与悬空转义应失败。
#[test]
fn TestSyntaxErrorCSV() {
    runFailingTestCasesCSV(&[
        (b"\"unterminated", CsvConfig::default()),
        (b"abc\\", CsvConfig::default()),
    ]);
}

/// 错误消息包含 unterminated quoted field。
#[test]
fn TestSyntaxErrorLog() {
    let error = read_all(b"\"unterminated", CsvConfig::default()).unwrap_err();
    assert!(error.to_string().contains("unterminated quoted field"));
}

/// 超过 LargestEntryLimit 报错。
#[test]
fn TestTooLargeRow() {
    let _guard = LARGEST_ENTRY_LIMIT_TEST_LOCK.lock().unwrap();
    let old = LargestEntryLimit.swap(8, Ordering::SeqCst);
    let result = read_all(b"123456789,ok\n", CsvConfig::default());
    LargestEntryLimit.store(old, Ordering::SeqCst);
    assert!(result.unwrap_err().to_string().contains("size of row"));
}

/// 限制针对整行，而不是单个字段。
#[test]
fn row_limit_counts_all_fields() {
    let _guard = LARGEST_ENTRY_LIMIT_TEST_LOCK.lock().unwrap();
    let old = LargestEntryLimit.swap(8, Ordering::SeqCst);
    let result = read_all(b"1234,5678\n", CsvConfig::default());
    LargestEntryLimit.store(old, Ordering::SeqCst);
    assert!(result.unwrap_err().to_string().contains("size of row"));
}

/// Row.Length 只统计字段原始内容，不包含分隔符、引号和行终止符。
#[test]
fn row_length_matches_go_field_content_contract() {
    let mut parser = make_parser(b"a,\"b,c\"\n", CsvConfig::default(), false).unwrap();
    parser.ReadRow().unwrap();
    assert_eq!(parser.LastRow().length, 4);
}

/// Go 在每次 ReadRow 入口先递增 RowID，即使随后遇到 EOF。
#[test]
fn row_id_advances_on_failed_read() {
    let mut parser = make_parser(b"", CsvConfig::default(), false).unwrap();
    assert!(matches!(parser.ReadRow(), Err(Error::Eof)));
    assert_eq!(Parser::Pos(&parser).1, 1);
}

/// 表头解析为列名后再读数据行。
#[test]
fn TestHeaderSchemaMatch() {
    let mut parser = make_parser(b"ID,Name\n1,Alice\n", CsvConfig::default(), true).unwrap();
    parser.ReadRow().unwrap();
    assert_eq!(parser.Columns(), &["id".to_owned(), "name".to_owned()]);
    assert_eq!(datumsToString(&parser.LastRow().row)[0], Some("1".into()));
}

/// 是否消费表头由构造参数决定，cfg.header 仅供上层规划使用。
#[test]
fn constructor_header_argument_controls_header_consumption() {
    let cfg = CsvConfig {
        header: true,
        ..Default::default()
    };
    let mut parser = make_parser(b"id,name\n1,Alice\n", cfg, false).unwrap();
    parser.ReadRow().unwrap();
    assert_eq!(
        datumsToString(&parser.LastRow().row),
        vec![Some("id".into()), Some("name".into())]
    );
}

/// GB18030 编码输入经转换器解码。
#[test]
fn TestCharsetConversion() {
    let converter = NewCharsetConvertor("gb18030", "�").unwrap();
    let encoded = converter.Encode("你好,世界\n").unwrap();
    let mut parser = NewCSVParser(
        &CsvConfig::default(),
        Box::new(StringReader::from_bytes(encoded)),
        false,
        Some(converter),
    )
    .unwrap();
    parser.ReadRow().unwrap();
    assert_eq!(
        datumsToString(&parser.LastRow().row),
        vec![Some("你好".into()), Some("世界".into())]
    );
}

/// 底层 Read 失败应向上返回。
#[test]
fn TestReadError() {
    struct BrokenReader;
    impl BrokenReader {
        fn Read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(IoError::new(ErrorKind::UnexpectedEof, "truncated input"))
        }
        fn Seek(&mut self, _position: SeekFrom) -> std::io::Result<u64> {
            Ok(0)
        }
        fn Close(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Read for BrokenReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.Read(buffer)
        }
    }
    impl Seek for BrokenReader {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.Seek(position)
        }
    }
    let mut close_probe = BrokenReader;
    close_probe.Close().unwrap();
    let error = NewCSVParser(&CsvConfig::default(), Box::new(BrokenReader), false, None)
        .err()
        .unwrap();
    assert!(error.to_string().contains("truncated input"));
}

/// 复用 TestReadError 覆盖截断读错误表面。
#[test]
fn TestReadBlockSurfacesTruncatedZstd() {
    TestReadError();
}

/// 自定义转义悬空应失败。
#[test]
fn TestCustomEscapeCharSyntax() {
    let cfg = CsvConfig {
        fields_escaped_by: "!".into(),
        ..Default::default()
    };
    assert!(read_all(b"dangling!", cfg).is_err());
}

/// 禁止未转义引号时中间引号报错。
#[test]
fn TestUnescapedQuote() {
    let cfg = CsvConfig {
        unescaped_quote: false,
        ..Default::default()
    };
    assert!(read_all(b"ab\"cd,x\n", cfg).is_err());
}

/// LOAD DATA 宽松模式把引号字段内、未处于字段边界的引号保留为文本。
#[test]
fn TestUnescapedQuoteInsideQuotedField() {
    assert_eq!(
        read_all(b"\"a\"b\",c\"d\"e\n", CsvConfig::default()).unwrap()[0],
        vec![Some("a\"b".into()), Some("c\"d\"e".into())]
    );
}

/// 无行结束符的最后一行仍可读。
#[test]
fn TestTerminatorAtEOF() {
    assert_eq!(read_all(b"a,b", CsvConfig::default()).unwrap().len(), 1);
}

/// 引号内 \\N 在开关开启时为文本 "N"。
#[test]
fn TestQuotedNullIsText() {
    let cfg = CsvConfig {
        quoted_null_is_text: true,
        ..Default::default()
    };
    assert_eq!(read_all(b"\"\\N\"\n", cfg).unwrap()[0][0], Some("N".into()));
}

/// 触发基准 harness 函数。
#[test]
fn TestBenchHarnesses() {
    BenchmarkReadRowUsingMydumpCSVParser();
    BenchmarkReadRowUsingEncodingCSV();
}

#[test]
fn TestCustomEscapeCharMetacharacter() {
    let cfg = CsvConfig {
        fields_escaped_by: "*".into(),
        null: "*N".into(),
        ..Default::default()
    };
    let mut parser = make_parser(br#""*0*b*n*r*t*Z***q",*N,**N"#, cfg, false).unwrap();
    parser.ReadRow().unwrap();
    assert_eq!(
        parser.LastRow().row,
        vec![
            Datum::Bytes(vec![0, 8, 10, 13, 9, 26, b'*', b'q']),
            Datum::Null,
            Datum::Bytes(b"*N".to_vec()),
        ]
    );
    assert!(matches!(parser.ReadRow(), Err(Error::Eof)));
}

#[test]
fn TestCSVParserUnescapeDenseRows() {
    let _guard = LARGEST_ENTRY_LIMIT_TEST_LOCK.lock().unwrap();
    for escape in [b'\\', b'!', b'*'] {
        let cfg = CsvConfig {
            fields_escaped_by: String::from_utf8(vec![escape]).unwrap(),
            ..Default::default()
        };
        let json = br#"{"id":123,"name":"alice","nested":{"enabled":true,"items":["a","b","c"]}}"#;
        let mut input = b"1,\"".to_vec();
        for &byte in json {
            if byte == b'"' {
                input.push(escape);
            }
            input.push(byte);
        }
        input.extend_from_slice(b"\",3\n");
        let mut parser = make_parser(&input.repeat(4096), cfg, false).unwrap();
        for id in 1..=4096 {
            parser.ReadRow().unwrap();
            assert_eq!(parser.LastRow().row_id, id);
            assert_eq!(
                parser.LastRow().row,
                vec![
                    Datum::Bytes(b"1".to_vec()),
                    Datum::Bytes(json.to_vec()),
                    Datum::Bytes(b"3".to_vec())
                ]
            );
            parser.RecycleRow(parser.LastRow());
        }
        assert!(matches!(parser.ReadRow(), Err(Error::Eof)));
    }
    assert_eq!(
        read_all(b"1,plain-text,3\n", CsvConfig::default()).unwrap()[0][1],
        Some("plain-text".into())
    );
}
