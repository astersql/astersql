// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use astersql_dxf_framework_taskexecutor_execute::TestCollector;
use astersql_lightning_backend::{BackendError, ChunkFlushStatus, EngineWriter, Logger};
use astersql_lightning_backend_encode::{Context, Datum, EncodingConfig, Rows};
use astersql_lightning_backend_kv::{GroupedPairs, Pairs};
use astersql_lightning_mydump::{CsvConfig, NewCSVParser, NewStringReader};
use astersql_lightning_verification::{KvPair, NewKVGroupChecksumWithKeyspace};
use astersql_meta_model::{ColumnInfo, TableInfo, ast};

use super::*;

#[derive(Clone, Default)]
struct WriterState {
    calls: Arc<AtomicUsize>,
    rows: Arc<AtomicUsize>,
}

struct RecordingWriter {
    state: WriterState,
    error: Option<&'static str>,
}

impl EngineWriter for RecordingWriter {
    fn AppendRows(
        &mut self,
        _context: &Context,
        _column_names: &[String],
        rows: &dyn Rows,
    ) -> Result<(), BackendError> {
        self.state.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.error {
            return Err(BackendError::new(error));
        }
        let count = rows
            .as_any()
            .downcast_ref::<Pairs>()
            .map(|pairs| pairs.Pairs.len())
            .or_else(|| {
                rows.as_any()
                    .downcast_ref::<GroupedPairs>()
                    .map(|groups| groups.0.values().map(Vec::len).sum())
            })
            .expect("processor must pass KV rows to engine writers");
        self.state.rows.fetch_add(count, Ordering::SeqCst);
        Ok(())
    }

    fn IsSynced(&self) -> bool {
        true
    }

    fn Close(&mut self, _context: &Context) -> Result<Option<ChunkFlushStatus>, BackendError> {
        Ok(Some(ChunkFlushStatus { flushed: true }))
    }
}

fn table_encoder() -> TableKVEncoder {
    let mut column = ColumnInfo::default();
    column.ID = 1;
    column.Name = ast::NewCIStr("value");
    column.SetType(astersql_parser_mysql::r#type::TypeLonglong);
    let meta = TableInfo {
        ID: 42,
        Name: ast::NewCIStr("t"),
        Columns: vec![column],
        ..TableInfo::default()
    };
    let config = EncodingConfig {
        Table: Some(Arc::new(NewTableDefinitionFromMeta(&meta).unwrap())),
        ..EncodingConfig::default()
    };
    NewTableKVEncoderFromMeta(
        &config,
        &meta,
        Arc::new(CanonicalImportDatumConverter(
            astersql_types::StrictContext.Flags(),
        )),
    )
    .unwrap()
}

fn csv_parser(source: &str) -> Box<dyn astersql_lightning_mydump::Parser + Send> {
    Box::new(
        NewCSVParser(
            &CsvConfig::default(),
            Box::new(NewStringReader(source)),
            false,
            None,
        )
        .unwrap(),
    )
}

fn writer(state: &WriterState, error: Option<&'static str>) -> Box<dyn EngineWriter> {
    Box::new(RecordingWriter {
        state: state.clone(),
        error,
    })
}

#[test]
fn file_chunk_process_updates_writers_checksum_and_collector() {
    let source = "1\n2\n3\n";
    let data = WriterState::default();
    let index = WriterState::default();
    let checksum = Arc::new(Mutex::new(NewKVGroupChecksumWithKeyspace(&[])));
    let collector = Arc::new(TestCollector::default());
    let mut processor = NewFileChunkProcessor(
        csv_parser(source),
        table_encoder(),
        Vec::new(),
        "test.csv",
        0,
        source.len() as i64,
        writer(&data, None),
        writer(&index, None),
        Some(checksum.clone()),
        Some(collector.clone()),
    );
    processor.encoder.min_deliver_row_count = 2;

    processor.Process(&Context::default()).unwrap();

    assert_eq!(2, data.calls.load(Ordering::SeqCst));
    assert_eq!(3, data.rows.load(Ordering::SeqCst));
    assert_eq!(2, index.calls.load(Ordering::SeqCst));
    assert_eq!(0, index.rows.load(Ordering::SeqCst));
    assert_eq!((3, 0), checksum.lock().unwrap().DataAndIndexSumKVS());
    assert_eq!(
        source.len() as i64,
        collector.ReadBytes.load(Ordering::SeqCst)
    );
    assert_eq!(3, collector.Rows.load(Ordering::SeqCst));
    assert!(collector.ProcessedCnt.load(Ordering::SeqCst) > 0);
}

#[test]
fn file_chunk_process_propagates_encoding_and_csv_errors() {
    let mut invalid_value = NewFileChunkProcessor(
        csv_parser("1\nnot-an-integer\n3\n"),
        table_encoder(),
        Vec::new(),
        "test.csv",
        0,
        19,
        writer(&WriterState::default(), None),
        writer(&WriterState::default(), None),
        None,
        None,
    );
    let error = invalid_value.Process(&Context::default()).unwrap_err();
    assert!(error.contains("test.csv at source offset 2"), "{error}");

    let mut invalid_csv = NewFileChunkProcessor(
        csv_parser("1,\""),
        table_encoder(),
        Vec::new(),
        "invalid.csv",
        0,
        3,
        writer(&WriterState::default(), None),
        writer(&WriterState::default(), None),
        None,
        None,
    );
    let error = invalid_csv.Process(&Context::default()).unwrap_err();
    assert!(error.contains("invalid.csv at offset 0"), "{error}");
    assert!(error.contains("unterminated quoted field"), "{error}");
}

#[test]
fn file_chunk_process_propagates_data_and_index_writer_errors() {
    for (data_error, index_error, expected) in [
        (Some("data write error"), None, "data write error"),
        (None, Some("index write error"), "index write error"),
    ] {
        let mut processor = NewFileChunkProcessor(
            csv_parser("1\n"),
            table_encoder(),
            Vec::new(),
            "test.csv",
            0,
            2,
            writer(&WriterState::default(), data_error),
            writer(&WriterState::default(), index_error),
            None,
            None,
        );
        let error = processor.Process(&Context::default()).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn index_route_writer_propagates_factory_error() {
    let factory: WriterFactory = Arc::new(|_| Err("some err".to_owned()));
    let mut writer = NewIndexRouteWriter(Logger::default(), factory);
    let rows = GroupedPairs(std::collections::BTreeMap::from([(
        1,
        vec![KvPair::default()],
    )]));

    let error = writer
        .AppendRows(&Context::default(), &[], &rows)
        .unwrap_err();
    assert!(error.to_string().contains("some err"));
}

#[test]
fn query_reader_skips_empty_chunks_without_panicking() {
    let (sender, receiver) = std::sync::mpsc::channel();
    sender.send(QueryChunk::default()).unwrap();
    sender
        .send(QueryChunk {
            rows: vec![vec![Datum::Int(7)]],
            row_id_offset: 10,
        })
        .unwrap();
    drop(sender);

    let mut reader = queryRowEncodeReader(Arc::new(Mutex::new(receiver)));
    let row = reader.ReadRow(Vec::new()).unwrap().unwrap();
    assert_eq!(11, row.row_id);
    assert_eq!(vec![Datum::Int(7)], row.row);
    assert!(reader.ReadRow(Vec::new()).unwrap().is_none());
}

#[test]
fn file_chunk_process_logs_original_size_with_nonempty_indexed_rows() {
    use astersql_meta_model::{IndexColumn, IndexInfo, StatePublic};
    let columns = ["a", "b", "c"]
        .into_iter()
        .enumerate()
        .map(|(offset, name)| {
            let mut column = ColumnInfo {
                ID: offset as i64 + 1,
                Name: ast::NewCIStr(name),
                Offset: offset as isize,
                State: StatePublic,
                ..Default::default()
            };
            column.SetType(astersql_parser_mysql::r#type::TypeLong);
            column
        })
        .collect();
    let indices = [("a", vec![0]), ("bc", vec![1, 2])]
        .into_iter()
        .enumerate()
        .map(|(id, (name, offsets))| IndexInfo {
            ID: id as i64 + 1,
            Name: ast::NewCIStr(name),
            State: StatePublic,
            Columns: offsets
                .into_iter()
                .map(|offset| IndexColumn {
                    Name: ast::NewCIStr(["a", "b", "c"][offset]),
                    Offset: offset as isize,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
        .collect();
    let meta = TableInfo {
        ID: 42,
        Columns: columns,
        Indices: indices,
        ..Default::default()
    };
    let config = EncodingConfig {
        Table: Some(Arc::new(NewTableDefinitionFromMeta(&meta).unwrap())),
        ..Default::default()
    };
    let encoder = NewTableKVEncoderFromMeta(
        &config,
        &meta,
        Arc::new(CanonicalImportDatumConverter(
            astersql_types::StrictContext.Flags(),
        )),
    )
    .unwrap();
    let source = "1,2,3\n4,5,6\n7,8,9\n";
    let chunk = Chunk {
        Path: "test.csv".into(),
        EndOffset: source.len() as i64,
        RowIDMax: 10000,
        ..Default::default()
    };
    let (logger, logs) = astersql_lightning_log::testlogger::MakeTestLogger([]);
    let logger = logger.With([astersql_lightning_log::Field::string("task", "import")]);
    let data = WriterState::default();
    let index = WriterState::default();
    let checksum = Arc::new(Mutex::new(NewKVGroupChecksumWithKeyspace(&[])));
    let collector = Arc::new(TestCollector::default());
    let mut processor = NewFileChunkProcessor(
        csv_parser(source),
        encoder,
        Vec::new(),
        chunk.GetKey(),
        0,
        chunk.EndOffset,
        writer(&data, None),
        writer(&index, None),
        Some(checksum.clone()),
        Some(collector.clone()),
    )
    .WithChunkLogger(&chunk, &logger);
    processor.encoder.min_deliver_row_count = 2;
    processor.Process(&Context::default()).unwrap();
    assert_eq!(data.rows.load(Ordering::SeqCst), 3);
    assert_eq!(index.rows.load(Ordering::SeqCst), 6);
    assert_eq!(checksum.lock().unwrap().DataAndIndexSumKVS(), (3, 6));
    assert_eq!(collector.Rows.load(Ordering::SeqCst), 3);
    assert_eq!(collector.ReadBytes.load(Ordering::SeqCst), 18);
    let (data_bytes, index_bytes) = checksum.lock().unwrap().DataAndIndexSumSize();
    // Check real encoded accounting independently of this commit's source-byte log.
    assert_eq!(
        collector.ProcessedCnt.load(Ordering::SeqCst) as u64,
        data_bytes + index_bytes
    );
    let starts = logs
        .lines()
        .into_iter()
        .filter(|line| line.contains("process chunk start"))
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 1);
    assert!(starts[0].contains("\"chunkSize\":18"));
    assert!(starts[0].contains("\"key\":\"test.csv:0\""));
    assert!(starts[0].contains("\"task\":\"import\""));
}
