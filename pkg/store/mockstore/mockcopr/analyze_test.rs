// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::analyze::analyzeColumnsExec;
use crate::{
    AnalyzeRequest, AnalyzeType, CopError, DagRequest, Datum, KeyRange, KvPair, KvReader, Request,
    RequestPayload, coprHandler,
};

#[derive(Default)]
struct CountingReader {
    scans: AtomicUsize,
    rows: Vec<KvPair>,
}

impl KvReader for CountingReader {
    fn scan(
        &self,
        _ranges: &[KeyRange],
        _start_ts: u64,
        _descending: bool,
    ) -> Result<Vec<KvPair>, CopError> {
        self.scans.fetch_add(1, Ordering::SeqCst);
        Ok(self.rows.clone())
    }
}

fn analyze_request(ranges: Vec<KeyRange>) -> Request {
    Request {
        ranges,
        start_ts: 10,
        payload: RequestPayload::Analyze(AnalyzeRequest {
            analyze_type: AnalyzeType::Columns,
            bucket_size: 8,
            sample_size: 8,
        }),
    }
}

#[test]
fn analyze_request_guards_match_go() {
    let reader = Arc::new(CountingReader::default());
    let handler = coprHandler::new(reader.clone());

    let response = handler.handleCopAnalyzeRequest(&analyze_request(Vec::new()));
    assert_eq!(response, Default::default());
    assert_eq!(reader.scans.load(Ordering::SeqCst), 0);

    let non_analyze = Request {
        ranges: vec![KeyRange {
            start: b"a".to_vec(),
            end: b"z".to_vec(),
        }],
        start_ts: 10,
        payload: RequestPayload::Dag(DagRequest::default()),
    };
    assert_eq!(
        handler.handleCopAnalyzeRequest(&non_analyze),
        Default::default()
    );
    assert_eq!(reader.scans.load(Ordering::SeqCst), 0);
}

#[test]
fn analyze_columns_exec_matches_record_set_lifecycle() {
    let reader = Arc::new(CountingReader {
        scans: AtomicUsize::new(0),
        rows: Vec::new(),
    });
    let mut exec = analyzeColumnsExec::new(reader.clone(), Vec::new(), 10);
    let mut chunk = vec![vec![Datum::Int(7)]];

    exec.Next(&mut chunk).unwrap();
    assert!(chunk.is_empty(), "Next must reset the destination chunk");
    exec.Next(&mut chunk).unwrap();
    assert_eq!(reader.scans.load(Ordering::SeqCst), 1);
}

#[test]
fn analyze_columns_close_does_not_rewind_the_record_set() {
    let reader = Arc::new(CountingReader {
        scans: AtomicUsize::new(0),
        rows: vec![KvPair {
            key: b"a".to_vec(),
            value: vec![Datum::Int(1)],
            commit_ts: 1,
        }],
    });
    let mut exec = analyzeColumnsExec::new(reader.clone(), Vec::new(), 10);

    assert_eq!(exec.getNext().unwrap(), Some(vec![Datum::Int(1)]));
    exec.Close();
    assert_eq!(exec.getNext().unwrap(), None);
    assert_eq!(reader.scans.load(Ordering::SeqCst), 1);
}
