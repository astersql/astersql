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

use crate::backfilling_dist_executor::BackfillSubTaskMeta;
use crate::backfilling_import_cloud::IndexInfo;
use crate::backfilling_merge_temp::{
    MergeTemporaryIndexExecutor, PhysicalTableCatalog, TemporaryIndexInfo,
};
use crate::backfilling_operators::{
    TemporaryIndexMutation, TemporaryIndexRecord, TemporaryIndexStore, TemporaryStoreError,
};

#[derive(Default)]
struct MemoryStore;

impl TemporaryIndexStore for MemoryStore {
    fn apply_batch(
        &mut self,
        _mutations: &[TemporaryIndexMutation],
    ) -> Result<(), TemporaryStoreError> {
        Ok(())
    }
}

fn encode_cmp_i64(value: i64) -> [u8; 8] {
    ((value as u64) ^ (1_u64 << 63)).to_be_bytes()
}

fn temporary_index_key(table_id: i64, index_id: i64, suffix: u8) -> Vec<u8> {
    const TEMP_INDEX_PREFIX: i64 = 0x7fff_0000_0000_0000;
    let mut key = vec![b't'];
    key.extend_from_slice(&encode_cmp_i64(table_id));
    key.extend_from_slice(b"_i");
    key.extend_from_slice(&encode_cmp_i64(TEMP_INDEX_PREFIX | index_id));
    key.push(suffix);
    key
}

fn executor(global: bool) -> MergeTemporaryIndexExecutor<MemoryStore> {
    MergeTemporaryIndexExecutor::new(
        1,
        2,
        16,
        PhysicalTableCatalog {
            parent_physical_id: 10,
            partition_ids: vec![42],
            indexes: vec![TemporaryIndexInfo {
                info: IndexInfo {
                    id: 7,
                    name: "idx".into(),
                    unique: false,
                },
                global,
            }],
        },
        MemoryStore,
    )
}

#[test]
fn initialization_decodes_table_and_masked_index_from_start_key_like_go() {
    let mut executor = executor(false);
    let mut meta = BackfillSubTaskMeta::default();
    meta.physical_table_id = 999;
    meta.element_ids = vec![999];
    meta.legacy_sorted_kv_meta.start_key = temporary_index_key(42, 7, 1);
    meta.legacy_sorted_kv_meta.end_key = temporary_index_key(42, 7, 2);

    executor.initialize_by_meta(&meta).unwrap();

    assert_eq!(executor.physical_table_id, Some(42));
    assert_eq!(
        executor.index_info.as_ref().map(|index| index.info.id),
        Some(7)
    );

    executor.cleanup();
    assert_eq!(executor.physical_table_id, Some(42));
    assert_eq!(
        executor.index_info.as_ref().map(|index| index.info.id),
        Some(7)
    );
}

#[test]
fn global_index_metrics_and_totals_follow_go_collector_semantics() {
    let mut executor = executor(true);
    let mut meta = BackfillSubTaskMeta::default();
    meta.physical_table_id = 999;
    meta.legacy_sorted_kv_meta.start_key = temporary_index_key(42, 7, 1);
    meta.legacy_sorted_kv_meta.end_key = temporary_index_key(42, 7, 4);
    let records = vec![
        TemporaryIndexRecord {
            temporary_key: temporary_index_key(42, 7, 2),
            original_key: b"original-1".to_vec(),
            value: b"value".to_vec(),
            delete: false,
            skip: false,
        },
        TemporaryIndexRecord {
            temporary_key: temporary_index_key(42, 7, 3),
            original_key: b"original-2".to_vec(),
            value: Vec::new(),
            delete: false,
            skip: true,
        },
    ];

    let results = executor.run_subtask(3, &meta, &records).unwrap();

    assert_eq!(executor.physical_table_id, Some(10));
    assert_eq!(results[0].scan_count, 2);
    assert_eq!(results[0].add_count, 1);
    assert_eq!(executor.summary.row_count, 1);
    assert_eq!(executor.total_rows, 1);
    assert_eq!(executor.merge_metrics.get(&(999, 7)), Some(&1));
}

#[test]
fn constructor_and_partition_selection_preserve_go_inputs() {
    let mut zero_batch =
        MergeTemporaryIndexExecutor::new(1, 2, 0, PhysicalTableCatalog::default(), MemoryStore);
    assert_eq!(zero_batch.batch_count, 0);
    assert_eq!(zero_batch.worker.batch_count, 0);

    let mut meta = BackfillSubTaskMeta::default();
    meta.legacy_sorted_kv_meta.start_key = temporary_index_key(10, 7, 1);
    meta.legacy_sorted_kv_meta.end_key = temporary_index_key(10, 7, 2);
    assert_eq!(
        executor(false).initialize_by_meta(&meta),
        Err(crate::backfilling_merge_temp::MergeTemporaryIndexError::PartitionNotFound(10))
    );

    zero_batch.parent_table = PhysicalTableCatalog {
        parent_physical_id: 10,
        partition_ids: Vec::new(),
        indexes: vec![TemporaryIndexInfo {
            info: IndexInfo {
                id: 7,
                name: "idx".into(),
                unique: false,
            },
            global: false,
        }],
    };
    meta.legacy_sorted_kv_meta.start_key = temporary_index_key(42, 7, 1);
    meta.legacy_sorted_kv_meta.end_key = temporary_index_key(42, 7, 2);
    zero_batch.initialize_by_meta(&meta).unwrap();
    assert_eq!(zero_batch.physical_table_id, Some(10));
}
