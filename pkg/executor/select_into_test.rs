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

// `SELECT INTO OUTFILE` 浮点格式化单元测试。
//
// 验证 `DumpRealOutfile` 在普通量级使用十进制输出，在极大/极小量级切换为科学计数法。

use std::io;
use std::sync::{Arc, Mutex};

use crate::select_into::{
    DumpRealOutfile, LineFieldsInfo, SelectIntoContext, SelectIntoExec, SelectIntoSink,
    SelectIntoSource,
};
use astersql_errors as errors;
use astersql_parser_ast::{SelectIntoOption, SelectIntoType};
use astersql_parser_mysql::r#type::{TypeLong, TypeVarchar};
use astersql_types::datum::FieldType;
use astersql_types::field::NewFieldType;
use astersql_util_chunk as chunk;

/// 与 Go `TestDumpReal` 的精度、阈值和科学计数法矩阵保持一致。
#[test]
fn select_into_real_formatter_matches_go_precision_and_thresholds() {
    let cases = [
        (1.2, 1, "1.2"),
        (1.2, 2, "1.20"),
        (2.0, 2, "2.00"),
        (2.333, -1, "2.333"),
        (1e14, -1, "100000000000000"),
        (1e15, -1, "1e15"),
        (1e-15, -1, "0.000000000000001"),
        (1e-16, -1, "1e-16"),
    ];

    for (value, decimal, expected) in cases {
        let mut field = FieldType::default();
        field.SetDecimal(decimal);
        let (_, actual) = DumpRealOutfile(Vec::new(), Vec::new(), value, &field);
        assert_eq!(String::from_utf8(actual).unwrap(), expected);
    }
}

/// Go `strconv.AppendFloat` preserves the SQL-facing spellings of non-finite values.
#[test]
fn select_into_real_formatter_matches_go_non_finite_spellings() {
    let field = FieldType::default();
    for (value, expected) in [
        (f64::INFINITY, "Inf"),
        (f64::NEG_INFINITY, "-Inf"),
        (f64::NAN, "NaN"),
    ] {
        let (_, actual) = DumpRealOutfile(Vec::new(), Vec::new(), value, &field);
        assert_eq!(String::from_utf8(actual).unwrap(), expected);
    }

    let mut fixed = FieldType::default();
    fixed.SetDecimal(2);
    let (_, actual) = DumpRealOutfile(Vec::new(), Vec::new(), f64::INFINITY, &fixed);
    assert_eq!(actual, b"+Inf");
}

#[derive(Default)]
struct PrefixTestSource;

impl SelectIntoSource for PrefixTestSource {
    fn Open(&mut self, _ctx: SelectIntoContext) -> Result<(), errors::SharedError> {
        Ok(())
    }

    fn Next(
        &mut self,
        _ctx: SelectIntoContext,
        _chunk: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError> {
        Ok(())
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        Ok(())
    }

    fn NewCacheChunk(&mut self) -> Box<chunk::Chunk> {
        Box::new(chunk::Chunk::default())
    }

    fn FieldTypes(&self) -> Vec<FieldType> {
        vec![*NewFieldType(TypeLong), *NewFieldType(TypeVarchar)]
    }

    fn AddAffectedRows(&mut self, _rows: u64) {}
}

struct PrefixTestSink(Arc<Mutex<Vec<u8>>>);

impl SelectIntoSink for PrefixTestSink {
    fn WriteAll(&mut self, data: &[u8]) -> io::Result<()> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(())
    }

    fn Flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn Close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn dump_rows_with_lines_starting_by(
    prefix: &str,
    enclosed_by: &str,
    escaped_by: &str,
    line_terminator: &str,
) -> Vec<u8> {
    let field_types = vec![*NewFieldType(TypeLong), *NewFieldType(TypeVarchar)];
    let mut rows = chunk::New(field_types.clone(), 3, 3);
    rows.AppendInt64(0, 1);
    rows.AppendString(1, "x");
    rows.AppendInt64(0, 2);
    rows.AppendString(1, "y");
    rows.AppendInt64(0, 3);
    rows.AppendNull(1);

    let written = Arc::new(Mutex::new(Vec::new()));
    let mut executor = SelectIntoExec {
        source: Box::new(PrefixTestSource),
        intoOpt: Box::new(SelectIntoOption {
            Tp: SelectIntoType::Outfile,
            ..SelectIntoOption::default()
        }),
        LineFieldsInfo: LineFieldsInfo {
            FieldsTerminatedBy: ",".to_owned(),
            FieldsEnclosedBy: enclosed_by.to_owned(),
            FieldsEscapedBy: escaped_by.to_owned(),
            FieldsOptEnclosed: false,
            LinesStartingBy: prefix.to_owned(),
            LinesTerminatedBy: line_terminator.to_owned(),
        },
        lineBuf: Vec::new(),
        realBuf: Vec::new(),
        fieldBuf: Vec::new(),
        escapeBuf: Vec::new(),
        enclosed: false,
        writer: Some(Box::new(PrefixTestSink(Arc::clone(&written)))),
        chk: Some(rows),
        fieldTypes: field_types,
        started: true,
    };

    executor.dumpToOutfile().unwrap();
    let result = written.lock().unwrap().clone();
    result
}

/// Go `TestSelectIntoOutfileLinesStartingBy`: every row receives the literal prefix,
/// including rows whose first field is NULL, without applying field escaping to it.
#[test]
fn select_into_outfile_honors_lines_starting_by() {
    assert_eq!(
        dump_rows_with_lines_starting_by("PREFIX", "", "\\", "\n"),
        b"PREFIX1,x\nPREFIX2,y\nPREFIX3,\\N\n"
    );
    assert_eq!(
        dump_rows_with_lines_starting_by("#;\"", "\"", "#", "END"),
        b"#;\"\"1\",\"x\"END#;\"\"2\",\"y\"END#;\"\"3\",#NEND"
    );
    assert_eq!(
        dump_rows_with_lines_starting_by("", "", "\\", "\n"),
        b"1,x\n2,y\n3,\\N\n"
    );
}
