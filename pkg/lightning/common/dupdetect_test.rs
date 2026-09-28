// Copyright 2026 AsterSQL.

use super::*;
use std::sync::{Arc, Mutex};

struct SliceIter {
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    index: usize,
}

impl KVIter for SliceIter {
    fn Next(&mut self) -> bool {
        self.index += 1;
        self.index < self.entries.len()
    }

    fn Key(&self) -> &[u8] {
        &self.entries[self.index].0
    }

    fn Value(&self) -> &[u8] {
        &self.entries[self.index].1
    }
}

#[derive(Default)]
struct BatchState {
    pending: Vec<(Vec<u8>, Vec<u8>)>,
    committed: Vec<(Vec<u8>, Vec<u8>)>,
    sync_commits: usize,
    closed: bool,
}

struct InspectBatch(Arc<Mutex<BatchState>>);

impl WriteBatch for InspectBatch {
    fn Set(&mut self, key: &[u8], value: &[u8]) -> Result<(), CommonError> {
        self.0
            .lock()
            .unwrap()
            .pending
            .push((key.to_vec(), value.to_vec()));
        Ok(())
    }

    fn Commit(&mut self, sync: bool) -> Result<(), CommonError> {
        let mut state = self.0.lock().unwrap();
        if sync {
            state.sync_commits += 1;
        }
        let pending = std::mem::take(&mut state.pending);
        state.committed.extend(pending);
        Ok(())
    }

    fn Reset(&mut self) {
        self.0.lock().unwrap().pending.clear();
    }

    fn Close(&mut self) -> Result<(), CommonError> {
        self.0.lock().unwrap().closed = true;
        Ok(())
    }
}

#[derive(Default)]
struct LogState(Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>);

struct InspectLogger(Arc<Mutex<LogState>>);

impl DupDetectLogger for InspectLogger {
    fn DuplicateDetected(&self, key: &[u8], value: &[u8], raw_key: &[u8]) {
        self.0
            .lock()
            .unwrap()
            .0
            .push((key.to_vec(), value.to_vec(), raw_key.to_vec()));
    }
}

#[test]
fn duplicate_records_both_rows_logs_and_syncs_on_close() {
    let entries = vec![
        (b"a".to_vec(), b"v1".to_vec()),
        (b"a".to_vec(), b"v2".to_vec()),
        (b"b".to_vec(), b"v3".to_vec()),
    ];
    let mut iter = SliceIter { entries, index: 0 };
    let batch = Arc::new(Mutex::new(BatchState::default()));
    let logs = Arc::new(Mutex::new(LogState::default()));
    let mut detector = NewDupDetector(
        Arc::new(NoopKeyAdapter),
        Box::new(InspectBatch(batch.clone())),
        Arc::new(InspectLogger(logs.clone())),
        DupDetectOpt::default(),
    );

    assert_eq!(
        detector.Init(&iter).unwrap(),
        (b"a".to_vec(), b"v1".to_vec())
    );
    assert_eq!(
        detector.Next(&mut iter).unwrap(),
        Some((b"b".to_vec(), b"v3".to_vec()))
    );
    detector.Close().unwrap();

    let state = batch.lock().unwrap();
    assert_eq!(
        state.committed,
        vec![
            (b"a".to_vec(), b"v1".to_vec()),
            (b"a".to_vec(), b"v2".to_vec())
        ]
    );
    assert_eq!(state.sync_commits, 1);
    assert!(state.closed);
    assert_eq!(logs.lock().unwrap().0.len(), 2);
}

#[test]
fn report_error_does_not_record_duplicate() {
    let entries = vec![
        (b"a".to_vec(), b"v1".to_vec()),
        (b"a".to_vec(), b"v2".to_vec()),
    ];
    let mut iter = SliceIter { entries, index: 0 };
    let batch = Arc::new(Mutex::new(BatchState::default()));
    let logs = Arc::new(Mutex::new(LogState::default()));
    let mut detector = NewDupDetector(
        Arc::new(NoopKeyAdapter),
        Box::new(InspectBatch(batch.clone())),
        Arc::new(InspectLogger(logs.clone())),
        DupDetectOpt {
            ReportErrOnDup: true,
        },
    );

    detector.Init(&iter).unwrap();
    let err = detector.Next(&mut iter).unwrap_err();
    assert!(err.to_string().contains("a"));
    assert!(batch.lock().unwrap().pending.is_empty());
    assert!(logs.lock().unwrap().0.is_empty());
}
