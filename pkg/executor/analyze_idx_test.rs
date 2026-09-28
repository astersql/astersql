// Copyright 2026 AsterSQL.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::analyze_idx::{
    AnalyzeIndexBackend, AnalyzeIndexError, AnalyzeIndexExec, AnalyzeIndexOptions,
    AnalyzeIndexResponse, AnalyzeResultStream, Bucket, CMSketch, Datum, FMSketch, Histogram,
    IndexInfo, Range, TopN,
};

struct ClosingStream {
    closed: Arc<AtomicBool>,
    close_error: Option<AnalyzeIndexError>,
}

struct ResponseStream {
    responses: VecDeque<AnalyzeIndexResponse>,
}

impl AnalyzeResultStream for ResponseStream {
    fn next_response(&mut self) -> Result<Option<AnalyzeIndexResponse>, AnalyzeIndexError> {
        Ok(self.responses.pop_front())
    }

    fn close(&mut self) -> Result<(), AnalyzeIndexError> {
        Ok(())
    }
}

struct ResponseBackend {
    responses: Mutex<Option<VecDeque<AnalyzeIndexResponse>>>,
}

impl AnalyzeIndexBackend for ResponseBackend {
    fn open_index_result(
        &self,
        _index: &IndexInfo,
        _ranges: &[Range],
        _common_handle: bool,
        _null_range: bool,
        _snapshot: Option<u64>,
        _concurrency: usize,
    ) -> Result<Box<dyn AnalyzeResultStream>, AnalyzeIndexError> {
        Ok(Box::new(ResponseStream {
            responses: self.responses.lock().unwrap().take().unwrap_or_default(),
        }))
    }

    fn killed(&self) -> Result<(), AnalyzeIndexError> {
        Ok(())
    }

    fn update_job_progress(&self, _rows: i64) {}
}

fn response_executor(responses: Vec<AnalyzeIndexResponse>) -> AnalyzeIndexExec<ResponseBackend> {
    AnalyzeIndexExec {
        backend: ResponseBackend {
            responses: Mutex::new(Some(responses.into())),
        },
        table_id: 1,
        idxInfo: IndexInfo {
            id: 2,
            column_count: 2,
            primary: false,
            multi_valued: false,
            global: false,
        },
        isCommonHandle: false,
        result: None,
        countNullRes: None,
        options: AnalyzeIndexOptions {
            buckets: 8,
            topn: 8,
            cms_depth: 1,
            cms_width: 1,
        },
        stats_version: 2,
        snapshot: 0,
        enable_snapshot: false,
        concurrency: 1,
    }
}

impl AnalyzeResultStream for ClosingStream {
    fn next_response(&mut self) -> Result<Option<AnalyzeIndexResponse>, AnalyzeIndexError> {
        Ok(None)
    }

    fn close(&mut self) -> Result<(), AnalyzeIndexError> {
        self.closed.store(true, Ordering::SeqCst);
        match &self.close_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

struct ClosingBackend {
    opens: AtomicUsize,
    main_closed: Arc<AtomicBool>,
    null_closed: Arc<AtomicBool>,
}

impl AnalyzeIndexBackend for ClosingBackend {
    fn open_index_result(
        &self,
        _index: &IndexInfo,
        _ranges: &[Range],
        _common_handle: bool,
        _null_range: bool,
        _snapshot: Option<u64>,
        _concurrency: usize,
    ) -> Result<Box<dyn AnalyzeResultStream>, AnalyzeIndexError> {
        let is_main = self.opens.fetch_add(1, Ordering::SeqCst) == 0;
        Ok(Box::new(ClosingStream {
            closed: if is_main {
                Arc::clone(&self.main_closed)
            } else {
                Arc::clone(&self.null_closed)
            },
            close_error: is_main.then(|| AnalyzeIndexError::Backend("main close".into())),
        }))
    }

    fn killed(&self) -> Result<(), AnalyzeIndexError> {
        Ok(())
    }

    fn update_job_progress(&self, _rows: i64) {}
}

#[test]
fn build_stats_closes_all_results_when_main_close_fails() {
    let main_closed = Arc::new(AtomicBool::new(false));
    let null_closed = Arc::new(AtomicBool::new(false));
    let mut executor = AnalyzeIndexExec {
        backend: ClosingBackend {
            opens: AtomicUsize::new(0),
            main_closed: Arc::clone(&main_closed),
            null_closed: Arc::clone(&null_closed),
        },
        table_id: 1,
        idxInfo: IndexInfo {
            id: 2,
            column_count: 1,
            primary: false,
            multi_valued: false,
            global: false,
        },
        isCommonHandle: false,
        result: None,
        countNullRes: None,
        options: AnalyzeIndexOptions::default(),
        stats_version: 2,
        snapshot: 0,
        enable_snapshot: false,
        concurrency: 1,
    };

    assert_eq!(
        executor.buildStats(&[Range::full_not_null()], true),
        Err(AnalyzeIndexError::Backend("main close".into()))
    );
    assert!(main_closed.load(Ordering::SeqCst));
    assert!(
        null_closed.load(Ordering::SeqCst),
        "Go closeAll closes the NULL result even after the main close fails"
    );
}

#[test]
fn build_stats_matches_go_topn_removal_and_v2_standardization() {
    let mut topn_values = BTreeMap::new();
    topn_values.insert(b"a".to_vec(), 2);
    let response = AnalyzeIndexResponse {
        histogram: Histogram {
            id: 0,
            ndv: 7,
            null_count: 0,
            buckets: vec![
                Bucket {
                    lower: Datum::Bytes(b"a".to_vec()),
                    upper: Datum::Bytes(b"a".to_vec()),
                    count: 5,
                    repeats: 5,
                },
                Bucket {
                    lower: Datum::Bytes(b"b".to_vec()),
                    upper: Datum::Bytes(b"b".to_vec()),
                    count: 5,
                    repeats: 0,
                },
            ],
        },
        cms: Some(CMSketch {
            depth: 1,
            width: 1,
            ..CMSketch::default()
        }),
        fm: Some(FMSketch {
            hashes: BTreeSet::from([11]),
        }),
        topn: Some(TopN {
            values: topn_values,
        }),
    };
    let mut executor = response_executor(vec![response]);

    let (histogram, _, _, _) = executor.buildStats(&[Range::full()], false).unwrap();

    assert_eq!(histogram.id, 2);
    assert_eq!(histogram.ndv, 7, "FM NDV must not overwrite histogram NDV");
    assert_eq!(histogram.buckets.len(), 1);
    assert_eq!(histogram.buckets[0].count, 3);
    assert_eq!(histogram.buckets[0].repeats, 0);
}

#[test]
fn build_stats_offsets_cumulative_counts_when_merging_responses() {
    let make_response = |value: u8, count: i64| AnalyzeIndexResponse {
        histogram: Histogram {
            ndv: 1,
            buckets: vec![Bucket {
                lower: Datum::Bytes(vec![value]),
                upper: Datum::Bytes(vec![value]),
                count,
                repeats: count,
            }],
            ..Histogram::default()
        },
        ..AnalyzeIndexResponse::default()
    };
    let mut executor = response_executor(vec![make_response(b'a', 2), make_response(b'b', 3)]);

    let (histogram, _, _, _) = executor.buildStats(&[Range::full()], false).unwrap();

    assert_eq!(histogram.ndv, 2);
    assert_eq!(histogram.buckets.last().unwrap().count, 5);
}
