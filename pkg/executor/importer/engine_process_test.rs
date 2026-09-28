// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use astersql_lightning_backend::{BackendError, ChunkFlushStatus, EngineWriter};
use astersql_lightning_backend_encode::{Context, EncodingConfig, Rows};
use astersql_lightning_mydump::{CsvConfig, NewCSVParser, NewStringReader};
use astersql_meta_model::{ColumnInfo, TableInfo, ast};

use super::*;

#[derive(Clone, Default)]
struct CloseState(Arc<AtomicUsize>);

struct LifecycleWriter {
    close_state: CloseState,
    append_error: Option<&'static str>,
    close_error: Option<&'static str>,
}

impl EngineWriter for LifecycleWriter {
    fn AppendRows(
        &mut self,
        _context: &Context,
        _column_names: &[String],
        _rows: &dyn Rows,
    ) -> Result<(), BackendError> {
        self.append_error
            .map_or(Ok(()), |error| Err(BackendError::new(error)))
    }

    fn IsSynced(&self) -> bool {
        true
    }

    fn Close(&mut self, _context: &Context) -> Result<Option<ChunkFlushStatus>, BackendError> {
        self.close_state.0.fetch_add(1, Ordering::SeqCst);
        self.close_error
            .map_or(Ok(Some(ChunkFlushStatus { flushed: true })), |error| {
                Err(BackendError::new(error))
            })
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

fn processor(
    data_close: &CloseState,
    index_close: &CloseState,
    append_error: Option<&'static str>,
    close_error: Option<&'static str>,
) -> BaseChunkProcessor {
    let parser = NewCSVParser(
        &CsvConfig::default(),
        Box::new(NewStringReader("1\n")),
        false,
        None,
    )
    .unwrap();
    NewFileChunkProcessor(
        Box::new(parser),
        table_encoder(),
        Vec::new(),
        "input.csv",
        0,
        2,
        Box::new(LifecycleWriter {
            close_state: data_close.clone(),
            append_error,
            close_error,
        }),
        Box::new(LifecycleWriter {
            close_state: index_close.clone(),
            append_error: None,
            close_error: None,
        }),
        None,
        None,
    )
}

#[test]
fn processing_error_still_closes_both_writers_like_go_defers() {
    let data_close = CloseState::default();
    let index_close = CloseState::default();
    let mut processor = processor(&data_close, &index_close, Some("append failed"), None);

    let error = processor.Process(&Context::default()).unwrap_err();

    assert!(error.contains("append failed"));
    assert_eq!(data_close.0.load(Ordering::SeqCst), 1);
    assert_eq!(index_close.0.load(Ordering::SeqCst), 1);
}

#[test]
fn close_error_is_non_fatal_like_go_deferred_cleanup() {
    let data_close = CloseState::default();
    let index_close = CloseState::default();
    let mut processor = processor(&data_close, &index_close, None, Some("close failed"));

    assert_eq!(processor.Process(&Context::default()), Ok(()));
    assert_eq!(data_close.0.load(Ordering::SeqCst), 1);
    assert_eq!(index_close.0.load(Ordering::SeqCst), 1);
}
