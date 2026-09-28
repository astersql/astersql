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

use std::collections::HashMap;

use crate::backfilling_dist_executor::{
    BackfillDistExecutor, BackfillSubTaskMeta, ExecutorError, ExternalMetaStorage, MetaError,
    decode_backfill_subtask_meta, write_external_backfill_subtask_meta,
};
use crate::backfilling_read_index::SortedKvMeta;

#[derive(Default)]
struct MemoryStorage(HashMap<String, Vec<u8>>);

impl ExternalMetaStorage for MemoryStorage {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        self.0.get(path).cloned().ok_or_else(|| "missing".into())
    }

    fn write(&mut self, path: &str, value: &[u8]) -> Result<(), String> {
        self.0.insert(path.to_owned(), value.to_vec());
        Ok(())
    }
}

fn sample_meta() -> BackfillSubTaskMeta {
    BackfillSubTaskMeta {
        physical_table_id: 42,
        row_start: b"row-a".to_vec(),
        row_end: b"row-z".to_vec(),
        range_job_keys: vec![b"job".to_vec()],
        range_split_keys: vec![b"split".to_vec()],
        data_files: vec!["data".into()],
        stat_files: vec!["stat".into()],
        ts: 99,
        meta_groups: vec![SortedKvMeta {
            start_key: b"meta-a".to_vec(),
            end_key: b"meta-z".to_vec(),
            file_count: 2,
            total_kv_size: 123,
        }],
        element_ids: vec![7],
        legacy_sorted_kv_meta: SortedKvMeta {
            start_key: b"legacy-a".to_vec(),
            end_key: b"legacy-z".to_vec(),
            file_count: 1,
            total_kv_size: 12,
        },
        ..BackfillSubTaskMeta::default()
    }
}

#[test]
fn external_meta_round_trip_preserves_internal_and_external_fields() {
    let mut storage = MemoryStorage::default();
    let mut original = sample_meta();

    write_external_backfill_subtask_meta(Some(&mut storage), &mut original, "task/meta")
        .expect("write external fields");
    let internal = original.marshal();
    let restored = decode_backfill_subtask_meta(Some(&storage), &internal)
        .expect("merge internal and external fields");

    assert_eq!(original, restored);
}

#[test]
fn externally_stored_payload_does_not_recurse_through_its_own_path() {
    let mut storage = MemoryStorage::default();
    let mut original = sample_meta();

    write_external_backfill_subtask_meta(Some(&mut storage), &mut original, "task/meta")
        .expect("write external fields");
    let external = storage.0.get("task/meta").expect("external payload");
    let external_meta = BackfillSubTaskMeta::unmarshal(external).expect("decode payload");

    assert!(external_meta.external_path.is_empty());
    assert_eq!(0, external_meta.physical_table_id);
    assert_eq!(vec!["data".to_owned()], external_meta.data_files);
}

#[test]
fn only_missing_index_metadata_is_non_retryable_like_go() {
    assert!(!BackfillDistExecutor::is_retryable_error(
        &ExecutorError::IndexInfoNotFound(7)
    ));
    assert!(BackfillDistExecutor::is_retryable_error(
        &ExecutorError::UnknownStep
    ));
    assert!(BackfillDistExecutor::is_retryable_error(
        &ExecutorError::LocalImportHasNoWriteAndIngest
    ));
    assert!(BackfillDistExecutor::is_retryable_error(
        &ExecutorError::Decode(MetaError::Truncated)
    ));
}
