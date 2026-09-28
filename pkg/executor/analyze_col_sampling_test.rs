// Copyright 2026 AsterSQL.

use super::analyze_col_sampling::*;
use std::sync::{Arc, Mutex};

struct SamplingBackend {
    packets: Mutex<Vec<Vec<u8>>>,
    collector: RowSampleCollector,
    fail_collation: bool,
}

impl SamplingBackend {
    fn with_row(columns: Vec<Datum>, mem_size: i64, fail_collation: bool) -> Self {
        let mut collector = RowSampleCollector::new(8, columns.len() + 1);
        collector.count = 1;
        collector.mem_size = mem_size;
        collector.samples.push(ReservoirRowSampleItem {
            columns,
            handle: 0,
            priority: 1,
        });
        Self {
            packets: Mutex::new(vec![vec![1]]),
            collector,
            fail_collation,
        }
    }
}

impl AnalyzeSamplingBackend for SamplingBackend {
    fn open_sampling(&self, _: &[Range]) -> Result<(), AnalyzeError> {
        Ok(())
    }
    fn next_raw(&self) -> Result<Option<Vec<u8>>, AnalyzeError> {
        Ok(self.packets.lock().unwrap().pop())
    }
    fn close_sampling(&self) -> Result<(), AnalyzeError> {
        Ok(())
    }
    fn decode_collector(
        &self,
        _: &[u8],
        _: usize,
        _: usize,
    ) -> Result<RowSampleCollector, AnalyzeError> {
        Ok(self.collector.clone())
    }
    fn decode_column(&self, _: &ColumnInfo, datum: &Datum) -> Result<Datum, AnalyzeError> {
        Ok(datum.clone())
    }
    fn evaluate_virtual_columns(
        &self,
        _: &[ColumnInfo],
        _: &mut [Datum],
    ) -> Result<(), AnalyzeError> {
        Ok(())
    }
    fn build_handle(&self, _: &[Datum]) -> Result<i64, AnalyzeError> {
        Ok(1)
    }
    fn analyze_index_ndv(&self, _: &IndexInfo) -> Result<(FMSketch, i64), AnalyzeError> {
        Ok((FMSketch::default(), 0))
    }
    fn collate_key(&self, _: &ColumnInfo, value: &Datum) -> Result<Datum, AnalyzeError> {
        if self.fail_collation {
            Err(AnalyzeError::Backend("collation failed".into()))
        } else {
            Ok(value.clone())
        }
    }
    fn encode_index_value(&self, _: &IndexInfo, row: &[Datum]) -> Result<Datum, AnalyzeError> {
        let mut encoded = Vec::new();
        for datum in row {
            match datum {
                Datum::Null => {}
                Datum::Signed(value) => encoded.extend(value.to_be_bytes()),
                Datum::Unsigned(value) => encoded.extend(value.to_be_bytes()),
                Datum::Bytes(value) => encoded.extend(value),
                Datum::Text(value) => encoded.extend(value.as_bytes()),
            }
        }
        Ok(Datum::Bytes(encoded))
    }
    fn killed(&self) -> Result<(), AnalyzeError> {
        Ok(())
    }
}

fn executor(
    backend: SamplingBackend,
    columns: Vec<ColumnInfo>,
) -> AnalyzeColumnsExec<SamplingBackend> {
    AnalyzeColumnsExec {
        backend: Arc::new(backend),
        table_id: 1,
        columns,
        indexes: vec![IndexInfo {
            id: 9,
            columns: vec![
                IndexColumn {
                    offset: 0,
                    prefix_length: None,
                },
                IndexColumn {
                    offset: 1,
                    prefix_length: None,
                },
            ],
            unique: false,
            primary: false,
        }],
        sample_size: 8,
        sample_rate: 1.0,
        bucket_count: 8,
        topn_count: 0,
        samplingStatsConcurrency: 1,
        stats_version: 2,
        handle_unsigned: false,
        memTracker: Arc::new(MemoryTracker::default()),
    }
}

fn column(id: i64, string_type: bool) -> ColumnInfo {
    ColumnInfo {
        id,
        offset: (id - 1) as usize,
        virtual_generated: false,
        generated_stored: false,
        string_type,
        unique_single_column: false,
    }
}

#[test]
fn composite_index_keeps_value_when_each_component_is_within_limit() {
    let columns = vec![column(1, false), column(2, false)];
    let backend = SamplingBackend::with_row(
        vec![Datum::Bytes(vec![1; 600]), Datum::Bytes(vec![2; 600])],
        32,
        false,
    );
    let result = executor(backend, columns).analyzeColumnsPushDown();
    assert_eq!(result.error, None);
    assert_eq!(result.results[1].histograms[0].buckets.len(), 1);
}

#[test]
fn build_error_releases_root_collector_memory() {
    let columns = vec![column(1, true), column(2, false)];
    let backend =
        SamplingBackend::with_row(vec![Datum::Text("x".into()), Datum::Signed(2)], 32, true);
    let exec = executor(backend, columns);
    let result = exec.analyzeColumnsPushDown();
    assert_eq!(
        result.error,
        Some(AnalyzeError::Backend("collation failed".into()))
    );
    assert_eq!(exec.memTracker.bytes(), 0);
}
