// Copyright 2026 AsterSQL.

use super::load_data::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};

#[derive(Default)]
struct TestBackend {
    parser: Mutex<Option<Box<dyn DataParser>>>,
}

impl LoadDataBackend for TestBackend {
    fn init_data_files(&self, _: &LoadDataController) -> Result<(), LoadDataError> {
        Ok(())
    }
    fn remote_reader_infos(
        &self,
        _: &LoadDataController,
    ) -> Result<Vec<LoadDataReaderInfo>, LoadDataError> {
        Ok(Vec::new())
    }
    fn local_reader_info(
        &self,
        path: &str,
        _: Box<dyn LoadDataReader>,
    ) -> Result<LoadDataReaderInfo, LoadDataError> {
        Ok(LoadDataReaderInfo {
            path: path.into(),
            offset: 0,
            length: None,
        })
    }
    fn open_parser(&self, _: &LoadDataReaderInfo) -> Result<Box<dyn DataParser>, LoadDataError> {
        self.parser
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| LoadDataError::Backend("parser missing".into()))
    }
    fn close_controller(&self, _: &LoadDataController) -> Result<(), LoadDataError> {
        Ok(())
    }
    fn evaluate_assignment(
        &self,
        _: usize,
        _: &BTreeMap<String, Datum>,
    ) -> Result<Datum, LoadDataError> {
        Ok(Datum::Null)
    }
    fn normalize_row(&self, _: u64, row: Vec<Datum>) -> Result<Vec<Datum>, LoadDataError> {
        Ok(row)
    }
    fn current_timestamp(&self, _: &ColumnInfo) -> Datum {
        Datum::Timestamp(42)
    }
    fn begin_transaction(&self) -> Result<(), LoadDataError> {
        Ok(())
    }
    fn set_transaction_low_priority(&self) -> Result<(), LoadDataError> {
        Ok(())
    }
    fn add_record(&self, _: &[Datum], _: bool, _: Option<usize>) -> Result<(), LoadDataError> {
        Ok(())
    }
    fn batch_check_and_insert(
        &self,
        rows: &[Vec<Datum>],
        _: bool,
    ) -> Result<(u64, u64), LoadDataError> {
        Ok((rows.len() as u64, 0))
    }
    fn statement_commit(&self) -> Result<(), LoadDataError> {
        Ok(())
    }
    fn commit_transaction(&self) -> Result<(), LoadDataError> {
        Ok(())
    }
    fn rollback_transaction(&self) -> Result<(), LoadDataError> {
        Ok(())
    }
    fn allow_write_row_id(&self) -> bool {
        true
    }
    fn killed(&self) -> Result<(), LoadDataError> {
        Ok(())
    }
}

fn controller() -> LoadDataController {
    let column = ColumnInfo {
        name: "created_at".into(),
        generated: false,
        time_type: true,
        not_null: true,
        extra_handle: false,
    };
    LoadDataController {
        path: "input.csv".into(),
        restrictive: true,
        ignore_lines: 0,
        field_count: 1,
        field_mappings: vec![FieldMapping::Column(column.clone())],
        insert_columns: vec![column],
        assignment_count: 0,
        expression_warnings: Vec::new(),
        on_duplicate: OnDuplicateKeyHandling::Error,
        max_rows_in_batch: 1000,
        low_priority: false,
        shard_allocate_step: 0,
    }
}

fn worker(backend: Arc<TestBackend>) -> LoadDataWorker<TestBackend> {
    NewLoadDataWorker(
        backend,
        controller(),
        planInfo {
            ID: 1,
            Columns: Vec::new(),
            GenColExprs: Vec::new(),
        },
        "t".into(),
    )
    .unwrap()
}

#[test]
fn explicit_null_time_value_is_not_treated_as_a_missing_field() {
    let backend = Arc::new(TestBackend::default());
    let load_worker = worker(backend);
    let (mut encoder, _) = initEncodeCommitWorkers(&load_worker).unwrap();

    let row = encoder.parserData2TableData(&[Datum::Null]).unwrap();

    assert_eq!(row, vec![Datum::Null]);
}

struct CloseFailParser;

impl DataParser for CloseFailParser {
    fn read_row(&mut self) -> Result<Vec<Datum>, LoadDataError> {
        Err(LoadDataError::EndOfFile)
    }
    fn recycle_row(&mut self, _: Vec<Datum>) {}
    fn close(&mut self) -> Result<(), LoadDataError> {
        Err(LoadDataError::Backend("close failed".into()))
    }
}

#[test]
fn parser_close_error_is_non_fatal_like_go_deferred_cleanup() {
    let backend = Arc::new(TestBackend {
        parser: Mutex::new(Some(Box::new(CloseFailParser))),
    });
    let load_worker = worker(backend);
    let (mut encoder, _) = initEncodeCommitWorkers(&load_worker).unwrap();
    let (input_tx, input_rx) = mpsc::sync_channel(1);
    let (output_tx, _output_rx) = mpsc::sync_channel(1);
    input_tx
        .send(LoadDataReaderInfo {
            path: "input.csv".into(),
            offset: 0,
            length: None,
        })
        .unwrap();
    drop(input_tx);

    let result = encoder.processStream(
        input_rx,
        output_tx,
        &std::sync::atomic::AtomicBool::new(false),
    );

    assert_eq!(result, Ok(()));
}
