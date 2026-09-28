// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use std::collections::HashSet;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use astersql_lightning_mydump::{Datum, MydumpError, Parser, Row, SourceFileMeta};
use astersql_meta_model::{ColumnInfo, IndexColumn, IndexInfo, StatePublic, TableInfo, ast};
use astersql_parser_mysql::r#type::TypeLonglong;

use crate::{
    DataFormatCSV, KVSizeParserService, KVSizeSampleConfig, SampleFileImportKVSizeWithTableInfo,
    SampledKVSizeResult, sample_file_indices, validateKVSizeSampleConfig,
};

struct TestParser {
    rows: Vec<Row>,
    cursor: usize,
    position: i64,
    row_id: i64,
    closed: Arc<AtomicBool>,
    close_error: bool,
    columns: Vec<String>,
}

impl Parser for TestParser {
    fn Pos(&self) -> (i64, i64) {
        (self.position, self.row_id)
    }
    fn SetPos(&mut self, pos: i64, row: i64) -> Result<(), MydumpError> {
        self.position = pos;
        self.row_id = row;
        Ok(())
    }
    fn ScannedPos(&mut self) -> Result<i64, MydumpError> {
        Ok(self.position)
    }
    fn Close(&mut self) -> Result<(), MydumpError> {
        self.closed.store(true, Ordering::SeqCst);
        if self.close_error {
            Err(MydumpError::Io("mock close error".into()))
        } else {
            Ok(())
        }
    }
    fn ReadRow(&mut self) -> Result<(), MydumpError> {
        let Some(row) = self.rows.get(self.cursor) else {
            return Err(MydumpError::Eof);
        };
        self.position += row.length as i64;
        self.row_id = row.row_id;
        self.cursor += 1;
        Ok(())
    }
    fn LastRow(&self) -> Row {
        self.rows[self.cursor - 1].clone()
    }
    fn RecycleRow(&mut self, _row: Row) {}
    fn Columns(&self) -> &[String] {
        &self.columns
    }
    fn SetColumns(&mut self, columns: Vec<String>) {
        self.columns = columns
    }
    fn SetRowID(&mut self, id: i64) {
        self.row_id = id
    }
}

struct TestParserService {
    rows: Vec<Row>,
    closed: Vec<Arc<AtomicBool>>,
    close_error: bool,
}

impl KVSizeParserService for TestParserService {
    fn NewParser(
        &self,
        file: &SourceFileMeta,
        _config: &KVSizeSampleConfig,
    ) -> Result<Box<dyn Parser>, String> {
        let index = file.path.parse::<usize>().unwrap();
        Ok(Box::new(TestParser {
            rows: self.rows.clone(),
            cursor: 0,
            position: 0,
            row_id: 0,
            closed: Arc::clone(&self.closed[index]),
            close_error: self.close_error,
            columns: Vec::new(),
        }))
    }
}

fn indexed_table() -> TableInfo {
    let mut column = ColumnInfo {
        ID: 1,
        Name: ast::NewCIStr("a"),
        Offset: 0,
        State: StatePublic,
        ..ColumnInfo::default()
    };
    column.SetType(TypeLonglong);
    let indexed_column = IndexColumn {
        Name: ast::NewCIStr("a"),
        Offset: 0,
        ..IndexColumn::default()
    };
    let primary = IndexInfo {
        ID: 1,
        Name: ast::NewCIStr("PRIMARY"),
        State: StatePublic,
        Primary: true,
        Unique: true,
        Columns: vec![indexed_column.clone()],
        ..IndexInfo::default()
    };
    let secondary = IndexInfo {
        ID: 2,
        Name: ast::NewCIStr("idx_a"),
        State: StatePublic,
        Columns: vec![indexed_column],
        ..IndexInfo::default()
    };
    TableInfo {
        ID: 42,
        Name: ast::NewCIStr("t"),
        PKIsHandle: true,
        Columns: vec![column],
        Indices: vec![primary, secondary],
        ..TableInfo::default()
    }
}

fn sample_rows(count: usize, length: u64) -> Vec<Row> {
    (0..count)
        .map(|index| Row {
            row: vec![Datum::Bytes((index + 1).to_string().into_bytes())],
            row_id: index as i64 + 1,
            length,
        })
        .collect()
}

#[test]
fn total_kv_size_wraps_like_go_uint64_addition() {
    let sampled = SampledKVSizeResult {
        DataKVSize: u64::MAX,
        IndexKVSize: 1,
        ..SampledKVSizeResult::default()
    };
    assert_eq!(sampled.TotalKVSize(), 0);
}

#[test]
fn sampled_file_indices_are_unique_for_every_selected_file() {
    for file_count in [1, 2, 3, 10, 7_919, 15_838] {
        let selected = sample_file_indices(file_count);
        assert_eq!(selected.len(), file_count.min(3));
        assert_eq!(
            selected.iter().copied().collect::<HashSet<_>>().len(),
            selected.len()
        );
        assert!(selected.iter().all(|index| *index < file_count));
    }
}

#[test]
fn config_validation_matches_go_format_and_prefix_guards() {
    let valid = KVSizeSampleConfig {
        Format: DataFormatCSV.into(),
        ..KVSizeSampleConfig::default()
    };
    validateKVSizeSampleConfig(&valid).unwrap();
    let mut invalid = valid.clone();
    invalid.Format = "json".into();
    assert!(
        validateKVSizeSampleConfig(&invalid)
            .unwrap_err()
            .contains("unsupported import format")
    );
    invalid = valid;
    invalid.LineFieldsInfo.FieldsEnclosedBy = "\"".into();
    invalid.LineFieldsInfo.FieldsTerminatedBy = "\"x".into();
    assert!(
        validateKVSizeSampleConfig(&invalid)
            .unwrap_err()
            .contains("must not prefix")
    );
}

#[test]
fn metadata_sampler_limits_files_and_rows_and_closes_every_parser() {
    let closed = (0..4)
        .map(|_| Arc::new(AtomicBool::new(false)))
        .collect::<Vec<_>>();
    let service = TestParserService {
        rows: sample_rows(20, 10),
        closed: closed.clone(),
        close_error: false,
    };
    let files = (0..4)
        .map(|index| SourceFileMeta {
            path: index.to_string(),
            ..SourceFileMeta::default()
        })
        .collect::<Vec<_>>();
    let result = SampleFileImportKVSizeWithTableInfo(
        KVSizeSampleConfig {
            Format: DataFormatCSV.into(),
            ..KVSizeSampleConfig::default()
        },
        &indexed_table(),
        &files,
        b"ks",
        &service,
    )
    .unwrap();
    assert_eq!(result.SourceSize, 300);
    assert!(result.DataKVSize > 0);
    assert!(result.IndexKVSize > 0);
    assert_eq!(
        closed
            .iter()
            .filter(|flag| flag.load(Ordering::SeqCst))
            .count(),
        3
    );
}

#[test]
fn parser_close_error_is_non_fatal_like_go_deferred_cleanup() {
    let closed = vec![Arc::new(AtomicBool::new(false))];
    let service = TestParserService {
        rows: sample_rows(1, 10),
        closed: closed.clone(),
        close_error: true,
    };
    let result = SampleFileImportKVSizeWithTableInfo(
        KVSizeSampleConfig {
            Format: DataFormatCSV.into(),
            ..KVSizeSampleConfig::default()
        },
        &indexed_table(),
        &[SourceFileMeta {
            path: "0".into(),
            ..SourceFileMeta::default()
        }],
        &[],
        &service,
    );
    assert!(
        result.is_ok(),
        "close errors are warnings in the Go sampler: {result:?}"
    );
    assert!(closed[0].load(Ordering::SeqCst));
}
