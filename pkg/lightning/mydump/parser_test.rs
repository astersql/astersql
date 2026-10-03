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

// ChunkParser（SQL INSERT 行解析器）单元测试。
//
// 覆盖整数/NULL/布尔/十六进制与二进制字面量、跨块续读、嵌套括号行、
// 注释剥离、伪关键字字符串、语法错误与空输入。Chunk：按字节偏移切分的导入分片。

use crate::*;

/// 用给定块大小与反斜杠转义开关构造 ChunkParser。
fn parser(input: &str, block_size: i64, no_backslash_escapes: bool) -> ChunkParser {
    NewChunkParser(
        Box::new(NewStringReader(input)),
        block_size,
        None,
        no_backslash_escapes,
    )
}

/// 读完整文件直到 EOF，收集所有 Row。
fn collect(input: &str, block_size: i64, no_backslash_escapes: bool) -> Result<Vec<Row>, Error> {
    let mut parser = parser(input, block_size, no_backslash_escapes);
    let mut rows = Vec::new();
    loop {
        match parser.ReadRow() {
            Ok(()) => rows.push(parser.LastRow()),
            Err(Error::Eof) => return Ok(rows),
            Err(error) => return Err(error),
        }
    }
}

/// 在多种 block_size 下断言行数与期望一致。
fn runTestCases(cases: &[(&str, usize)]) {
    for &(input, expected_rows) in cases {
        for block_size in [1, 2, 7, 64] {
            assert_eq!(
                collect(input, block_size, false).unwrap().len(),
                expected_rows
            );
        }
    }
}

/// 断言非法输入在 ReadRow 时返回 Syntax 错误。
fn runFailingTestCases(cases: &[&str]) {
    for &input in cases {
        let mut parser = parser(input, 2, false);
        assert!(matches!(parser.ReadRow(), Err(Error::Syntax(_))));
    }
}

/// 解析含负数、NULL、TRUE/FALSE、0x/b' 字面量的多行 VALUES。
#[test]
fn TestReadRow() {
    let rows = collect(
        "INSERT INTO t VALUES (1,-2,'three'),(4,NULL,TRUE),(0x0a,b'11',FALSE);",
        3,
        false,
    )
    .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].row_id, 1);
    assert_eq!(
        rows[0].row,
        vec![
            Datum::I64(1),
            Datum::I64(-2),
            Datum::Bytes(b"three".to_vec())
        ]
    );
    assert_eq!(rows[1].row, vec![Datum::I64(4), Datum::Null, Datum::I64(1)]);
    assert_eq!(
        rows[2].row,
        vec![
            Datum::Binary(vec![10]),
            Datum::Binary(vec![3]),
            Datum::I64(0)
        ]
    );
}

/// ReadChunks 按最小字节阈值切分，校验 offset 连续与 row_id 衔接。
#[test]
fn TestReadChunks() {
    let mut parser = parser(
        "INSERT INTO t VALUES (1),(2),(3),(4),(5),(6),(7),(8),(9);",
        3,
        false,
    );
    let chunks = ReadChunks(&mut parser, 12).unwrap();
    assert!(chunks.len() >= 2);
    assert_eq!(chunks.first().unwrap().prev_row_id_max, 0);
    assert_eq!(chunks.last().unwrap().row_id_max, 9);
    for pair in chunks.windows(2) {
        assert_eq!(pair[0].end_offset, pair[1].offset);
        assert_eq!(pair[0].row_id_max, pair[1].prev_row_id_max);
    }
}

/// CONVERT(... USING UTF8MB4) 被词法器当注释剥离，嵌套括号行仍正确计数。
#[test]
fn TestNestedRow() {
    let rows = collect(
        "INSERT INTO t VALUES (CONVERT('x' USING UTF8MB4),1),(2,3);",
        2,
        false,
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].row_id, 1);
    assert_eq!(rows[1].row_id, 2);
}

/// 多种合法语法：反引号表名、裸 VALUES 行、注释、三类引号。
#[test]
fn TestVariousSyntax() {
    runTestCases(&[
        ("INSERT INTO foobar VALUES (1,2);", 1),
        ("INSERT INTO `foobar` VALUES (3,4);", 1),
        ("(7,-8,NULL,'9'),(b'10',0b11,0x12,x'13')", 2),
        ("/* comment */ (1); -- tail", 1),
        ("(\"a\",'b',c)", 1),
    ]);
}

/// 未闭合引号/注释等应报语法错误。
#[test]
fn TestSyntaxError() {
    runFailingTestCases(&[
        "('unterminated)",
        "/* unterminated",
        "('/invalid)",
        "(`identifier-is-not-a-value`)",
    ]);
}

/// 长字段跨任意小块边界时，结果应与大块一次读完一致。
#[test]
fn TestContinuation() {
    let input = "INSERT INTO t VALUES ('a very long value crossing every block boundary',123456789),(2,'z');";
    let expected = collect(input, 1024, false).unwrap();
    for size in 1..16 {
        assert_eq!(collect(input, size, false).unwrap(), expected);
    }
}

/// 引号内的 null/true/false/values 应作为普通字符串，而非关键字。
#[test]
fn TestPseudoKeywords() {
    let rows = collect(
        "INSERT INTO t VALUES ('null','true','false','values');",
        2,
        false,
    )
    .unwrap();
    assert_eq!(
        rows[0].row,
        vec![
            Datum::Bytes(b"null".to_vec()),
            Datum::Bytes(b"true".to_vec()),
            Datum::Bytes(b"false".to_vec()),
            Datum::Bytes(b"values".to_vec()),
        ]
    );
}

/// 非法十六/二进制数字与悬空反斜杠转义。
#[test]
fn TestMoreSyntaxError() {
    runFailingTestCases(&["(x'0g')", "(b'012')", "('a\\')"]);
}

/// 空文件、纯空白、仅注释应得到零行。
#[test]
fn TestMoreEmptyFiles() {
    for input in ["", "  \n;", "/* comment */", "-- comment"] {
        assert!(collect(input, 1, false).unwrap().is_empty());
    }
}

/// Go 的四态解析器会提取并小写化列名，同时在下一条无列清单的 INSERT 重置列名。
#[test]
fn insert_state_machine_tracks_columns_and_rejects_invalid_tokens() {
    let mut p = parser(
        "INSERT INTO `db`.`t` (`A`,\"B\",c) VALUES (1,2,3); INSERT t VALUES (4);",
        3,
        false,
    );
    p.ReadRow().unwrap();
    assert_eq!(p.Columns(), &["a", "b", "c"]);
    p.ReadRow().unwrap();
    assert!(p.Columns().is_empty());

    let mut invalid = parser("INSERT INTO t VALUES (1 VALUES 2)", 2, false);
    assert!(matches!(invalid.ReadRow(), Err(Error::Syntax(_))));
}

/// Go 在十进制整数超出 64 bit 时保留原文本，而不是让解析失败。
#[test]
fn oversized_integer_falls_back_to_text() {
    let rows = collect("(65555555555555555555555555555555555555555555)", 2, false).unwrap();
    assert_eq!(
        rows[0].row,
        vec![Datum::Bytes(
            b"65555555555555555555555555555555555555555555".to_vec()
        )]
    );
}

/// ReadChunks 只把 EOF 当作正常结束，语法错误必须原样传播。
#[test]
fn read_chunks_propagates_syntax_errors() {
    let mut p = parser("(1) ('unterminated)", 2, false);
    assert!(matches!(ReadChunks(&mut p, 1), Err(Error::Syntax(_))));
}

/// Go ReadUntil 在文件先于目标位置结束时成功返回。
#[test]
fn read_until_accepts_eof_before_target() {
    let mut p = parser("(1)", 2, false);
    ReadUntil(&mut p, i64::MAX).unwrap();
    assert_eq!(p.LastRow().row_id, 1);
}

#[test]
fn TestUnescapeBytePairs() {
    for escape in [b'\\', b'!', b'*'] {
        let input = [
            escape, b'0', escape, b'b', escape, b'n', escape, b'r', escape, b't', escape, b'Z',
            escape, escape, escape, b'q', 0xff, escape,
        ];
        assert_eq!(
            unescape(&input, 0, EscapeFlavor::MySql, escape),
            vec![0, 8, 10, 13, 9, 26, escape, b'q', 0xff, escape]
        );
        assert_eq!(unescape(&input, 0, EscapeFlavor::None, escape), input);
        assert_eq!(unescape(b"plain", 0, EscapeFlavor::MySql, escape), b"plain");
        assert_eq!(
            unescape(&[escape], 0, EscapeFlavor::MySql, escape),
            [escape]
        );
    }
}

#[test]
fn TestUnescapeDelimiterBeforeEscape() {
    assert_eq!(unescape(br#"\''"#, b'\'', EscapeFlavor::MySql, b'\\'), b"'");
    let rows = collect(r#"INSERT INTO t VALUES ('a\nb');"#, 1, false).unwrap();
    assert_eq!(rows[0].row, vec![Datum::Bytes(b"a\nb".to_vec())]);
}
