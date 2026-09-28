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

use crate::backfilling_operators::{
    IndexIngestWorker, IndexRecord, IndexRecordChunk, MemoryIndexWriter, TableScanTaskSource,
};

#[test]
fn invalid_checkpoint_falls_back_to_original_start_like_go() {
    let source = TableScanTaskSource {
        start_key: vec![10],
        end_key: vec![20],
        checkpoint_key: Some(vec![9]),
        ..TableScanTaskSource::default()
    };

    assert_eq!(
        Ok((vec![10], false)),
        source.adjust_start_key(vec![10], &[20])
    );
}

#[test]
fn ingest_result_reports_scanned_rows_after_partial_index_filtering() {
    let mut worker = IndexIngestWorker {
        writer: MemoryIndexWriter::default(),
        index_count: 1,
    };
    let chunk = IndexRecordChunk {
        task_id: 7,
        records: vec![IndexRecord {
            row_key: vec![1],
            index_key: vec![2],
            value: vec![3],
            matches_partial_index: false,
        }],
        done: true,
        table_scan_row_count: 1,
        condition_pushed: false,
        error: None,
    };

    let result = worker.write_chunk(&chunk).unwrap();
    assert_eq!(1, result.row_count);
    assert!(worker.writer.data.is_empty());
}
