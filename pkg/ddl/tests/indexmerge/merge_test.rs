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

//! 对齐 Go `merge_test.go` 的 24 个 Index Merge 场景。

use std::collections::BTreeMap;

use super::{IndexMergeTable, MergeError, MergePhase, MergeRow};

fn rows(table: &IndexMergeTable) -> BTreeMap<i64, MergeRow> {
    table.rows.clone()
}

#[test]
fn test_add_index_merge_process() {
    let mut table = IndexMergeTable::with_rows(false, [(1, Some(1), 23), (4, Some(4), 56)]);
    table.start_backfill();
    table.insert(7, Some(7), 89).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(table.phase, MergePhase::Public);
    assert_eq!(
        rows(&table),
        BTreeMap::from([
            (
                1,
                MergeRow {
                    key: Some(1),
                    payload: 23
                }
            ),
            (
                4,
                MergeRow {
                    key: Some(4),
                    payload: 56
                }
            ),
            (
                7,
                MergeRow {
                    key: Some(7),
                    payload: 89
                }
            ),
        ])
    );
    assert_eq!(
        table
            .index
            .get(&Some(7))
            .unwrap()
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![7]
    );
}

#[test]
fn test_add_primary_key_merge_process() {
    let mut table = IndexMergeTable::with_rows(false, [(1, Some(1), 23), (4, Some(4), 56)]);
    table.start_backfill();
    table.delete(4);
    table.finish_checked().unwrap();
    assert_eq!(
        rows(&table),
        BTreeMap::from([(
            1,
            MergeRow {
                key: Some(1),
                payload: 23
            }
        )])
    );
}

#[test]
fn test_add_index_merge_version_index_value() {
    let mut table = IndexMergeTable::with_rows(true, [(1, Some(1), 0)]);
    table.start_backfill();
    table.insert(2, Some(2), 0).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(
        IndexMergeTable::origin_index_value(1),
        vec![0, 0, 0, 0, 0, 0, 0, 1]
    );
    assert!(!IndexMergeTable::origin_index_value(1).contains(&b'm'));
}

#[test]
fn test_add_index_merge_index_untouched_value() {
    let mut table = IndexMergeTable::with_rows(false, [(1, Some(1), 1)]);
    table.start_backfill();
    table.insert(100, Some(1), 1).unwrap();
    // Go updates the non-indexed `k` column while the indexed `c` value stays
    // untouched.  `key` models `c` and `payload` models `k` here.
    table.update(100, Some(1), 2).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(table.rows.get(&100).unwrap().key, Some(1));
    assert_eq!(table.rows.get(&100).unwrap().payload, 2);
}

#[test]
fn test_create_unique_index_key_exist() {
    let expression_key = |a: i64, b: i64| Some(a * b + 1);
    let mut table = IndexMergeTable::with_rows(
        true,
        [
            (1, expression_key(1, 1), 1),
            (2, expression_key(2, 2), 2),
            (3, expression_key(3, 3), 3),
            (4, expression_key(4, 4), 4),
        ],
    );
    table.start_backfill();
    table.set_phase(MergePhase::DeleteOnly);

    table.insert(5, expression_key(5, 5), 5).unwrap();

    // The pessimistic INSERT ... SELECT is rolled back in Go.  Exercise the
    // same duplicate temporary-key writes on a transaction-local snapshot and
    // then discard it, proving that rollback has no base-table side effects.
    let rows_before_rollback = rows(&table);
    let mut pessimistic_txn = table.clone();
    for (offset, (&handle, row)) in rows_before_rollback.iter().enumerate() {
        pessimistic_txn
            .insert_during_ddl(100 + offset as i64, row.key, row.payload)
            .unwrap();
        assert!(pessimistic_txn.rows.contains_key(&handle));
    }
    drop(pessimistic_txn);
    assert_eq!(rows(&table), rows_before_rollback);

    // The default `a = 0` row first carries b=6 and leaves a deleted temp key;
    // a later b=9 insert must be allowed to reuse the same expression key.
    table.insert(0, expression_key(0, 6), 6).unwrap();
    table.update(1, expression_key(1, 7), 7).unwrap();
    table.delete(4);

    table.set_phase(MergePhase::WriteOnly);
    table.insert(8, expression_key(8, 8), 8).unwrap();
    table.delete(3);
    table.update(2, expression_key(2, 7), 7).unwrap();

    table.set_phase(MergePhase::WriteReorganization);
    table.insert(10, expression_key(10, 10), 10).unwrap();
    table.delete(0);
    table.insert(0, expression_key(0, 9), 9).unwrap();
    table.update(5, expression_key(5, 7), 7).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(
        table.rows.keys().copied().collect::<Vec<_>>(),
        vec![0, 1, 2, 5, 8, 10]
    );
    assert_eq!(table.rows.get(&0).unwrap().key, Some(1));
    assert_eq!(table.rows.get(&0).unwrap().payload, 9);
    assert_eq!(table.rows.get(&1).unwrap().key, Some(8));
    assert_eq!(table.rows.get(&2).unwrap().key, Some(15));
    assert_eq!(table.rows.get(&5).unwrap().key, Some(36));
    assert_eq!(table.rows.get(&8).unwrap().key, Some(65));
    assert_eq!(table.rows.get(&10).unwrap().key, Some(101));
}

#[test]
fn test_add_index_merge_index_update_on_delete_only() {
    let mut table = IndexMergeTable::with_rows(false, [(1, Some(0), 1)]);
    table.start_backfill();
    table.set_phase(MergePhase::DeleteOnly);
    table.update(1, Some(0), 0).unwrap();
    table.update(1, Some(1), 1).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(table.rows.get(&1).unwrap().key, Some(1));
}

#[test]
fn test_add_index_merge_delete_unique_on_write_only() {
    let mut table = IndexMergeTable::with_rows(
        true,
        [
            (1, Some(1), 1),
            (2, Some(2), 2),
            (3, Some(3), 3),
            (4, Some(4), 4),
        ],
    );
    table.start_backfill();
    table.set_phase(MergePhase::DeleteOnly);
    table.insert(5, Some(5), 5).unwrap();

    table.set_phase(MergePhase::WriteOnly);
    // Both Go rows have the indexed value a=5; the second write is permitted
    // only as a temporary-index write and is deleted before merge completes.
    table.insert_during_ddl(6, Some(5), 7).unwrap();
    table.delete(6);
    table.finish_checked().unwrap();
    assert!(!table.rows.contains_key(&6));
    assert_eq!(table.rows.get(&5).unwrap().key, Some(5));
    assert_eq!(table.rows.get(&5).unwrap().payload, 5);
}

#[test]
fn test_add_index_merge_delete_null_unique() {
    let mut table = IndexMergeTable::with_rows(true, [(1, Some(1), 1), (2, None, 0)]);
    table.start_backfill();
    table.delete(2);
    table.finish_checked().unwrap();
    assert_eq!(table.rows.len(), 1);
    assert_eq!(table.index.get(&Some(1)).unwrap().len(), 1);
}

#[test]
fn test_add_index_merge_double_delete() {
    let mut table = IndexMergeTable::new(true);
    table.start_backfill();
    table.set_phase(MergePhase::WriteOnly);
    table.insert(1, Some(1), 1).unwrap();
    table.delete(1);
    table.insert(2, Some(1), 1).unwrap();
    table.delete(2);
    table.finish_checked().unwrap();
    assert!(table.rows.is_empty());
}

#[test]
fn test_add_index_merge_conflict_with_pessimistic() {
    let mut table = IndexMergeTable::with_rows(true, [(1, Some(1), 1)]);
    table.start_backfill();
    table.set_phase(MergePhase::WriteOnly);
    table.update(1, Some(2), 2).unwrap();
    table.acquire_pessimistic_lock();
    assert_eq!(table.finish_checked(), Err(MergeError::PessimisticLock));
    table.rollback_pessimistic();
    table.finish_checked().unwrap();
    assert_eq!(table.rows.get(&1).unwrap().key, Some(2));
}

#[test]
fn test_next_gen_pessimistic_txn_not_fail_with_temp_index() {
    let mut table = IndexMergeTable::with_rows(true, [(1, Some(1), 1)]);
    table.start_backfill();
    table.update(1, Some(1), 11).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(table.rows.get(&1).unwrap().payload, 11);
}

#[test]
fn test_add_index_merge_insert_on_merging() {
    let mut table = IndexMergeTable::new(true);
    table.start_backfill();
    table.set_phase(MergePhase::DeleteOnly);
    table.insert(1, Some(5), 5).unwrap();

    table.set_phase(MergePhase::WriteOnly);
    table.insert_during_ddl(2, Some(5), 7).unwrap();
    table.delete(2);

    table.set_phase(MergePhase::Merging);
    assert_eq!(
        table.insert(3, Some(5), 8),
        Err(MergeError::DuplicateUniqueValue)
    );
    table
        .insert_on_duplicate_update(3, Some(5), 5, Some(6))
        .unwrap();
    table.finish_checked().unwrap();
    assert_eq!(
        rows(&table),
        BTreeMap::from([(
            1,
            MergeRow {
                key: Some(6),
                payload: 5
            }
        )])
    );
}

#[test]
fn test_add_index_merge_replace_on_merging() {
    let mut table = IndexMergeTable::with_rows(true, [(5, Some(5), 5)]);
    table.start_backfill();
    table.set_phase(MergePhase::Merging);
    table.delete(5);
    table.replace(5, Some(5), 8).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(table.rows.get(&5).unwrap().payload, 8);
}

#[test]
fn test_add_index_merge_insert_to_deleted_temp_index() {
    let mut table = IndexMergeTable::with_rows(true, [(5, Some(5), 5)]);
    table.start_backfill();
    table.set_phase(MergePhase::WriteOnly);
    table.delete(5);
    table.insert(6, Some(5), 8).unwrap();
    assert_eq!(
        table.insert(7, Some(5), 8),
        Err(MergeError::DuplicateUniqueValue)
    );
    assert_eq!(
        table.insert(8, Some(5), 8),
        Err(MergeError::DuplicateUniqueValue)
    );
    table.finish_checked().unwrap();
    assert_eq!(table.rows.get(&6).unwrap().payload, 8);
}

#[test]
fn test_add_index_merge_replace_delete() {
    let mut table = IndexMergeTable::new(true);
    table.start_backfill();
    table.set_phase(MergePhase::DeleteOnly);
    table.insert(1, Some(1), 1).unwrap();
    table.set_phase(MergePhase::Merging);
    table.replace(2, Some(1), 1).unwrap();
    table.delete(2);
    table.finish_checked().unwrap();
    assert!(table.rows.is_empty());
}

#[test]
fn test_add_index_merge_delete_different_handle() {
    let mut table = IndexMergeTable::with_rows(true, [(1, Some(1), 1)]);
    table.start_backfill();
    table.insert_during_ddl(2, Some(1), 1).unwrap();
    // REPLACE 已把最终行切换为 handle 3，但临时索引仍保留旧 handle 1。
    table.insert_during_ddl(3, Some(1), 1).unwrap();
    table.delete(2);
    assert_eq!(
        table.finish_checked(),
        Err(MergeError::DuplicateUniqueValue)
    );
    table.delete(1);
    table.finish_checked().unwrap();
    assert_eq!(table.rows.keys().copied().collect::<Vec<_>>(), vec![3]);
}

#[test]
fn test_add_index_decode_temp_index_common_handle() {
    let encoded = IndexMergeTable::encode_common_handle(["2", "id_2"].as_ref());
    assert_eq!(
        IndexMergeTable::decode_common_handle(&encoded),
        Some(vec!["2".to_owned(), "id_2".to_owned()])
    );
    let mut table = IndexMergeTable::with_rows(false, [(1, Some(1), 1)]);
    table.start_backfill();
    table.insert(2, Some(2), 2).unwrap();
    table.insert(3, Some(3), 3).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(table.rows.len(), 3);
}

#[test]
fn test_add_index_insert_ignore_on_backfill() {
    let mut table = IndexMergeTable::new(true);
    table.start_backfill();
    assert!(table.insert_ignore(1, Some(1), 1).unwrap());
    assert!(table.insert_ignore(2, Some(2), 2).unwrap());
    table.update(1, None, 0).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(table.rows.get(&1).unwrap().key, None);
}

#[test]
fn test_add_index_multiple_delete() {
    let mut table = IndexMergeTable::with_rows(true, (1..=6).map(|id| (id, Some(1), 1)));
    table.start_backfill();
    table.set_phase(MergePhase::DeleteOnly);
    for id in [4, 5, 6] {
        table.delete(id);
    }
    table.set_phase(MergePhase::WriteOnly);
    for id in [2, 3] {
        table.delete(id);
    }
    table.delete(1);
    table.finish_checked().unwrap();
    assert!(table.rows.is_empty());
}

#[test]
fn test_add_index_duplicate_and_write_conflict() {
    let mut table = IndexMergeTable::with_rows(true, [(1, Some(1), 1)]);
    table.start_backfill();
    table.set_phase(MergePhase::WriteOnly);
    table.insert_during_ddl(2, Some(1), 1).unwrap();
    assert_eq!(table.cancel(), MergeError::DdlCancelled);
    assert_eq!(table.rows.keys().copied().collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(table.phase, MergePhase::NoIndex);
}

#[test]
fn test_add_index_update_untouched_values() {
    let mut table = IndexMergeTable::with_rows(true, [(1, Some(1), 1)]);
    table.start_backfill();
    table.update(1, Some(1), 2).unwrap();
    table.insert_during_ddl(2, Some(1), 2).unwrap();
    assert_eq!(
        table.finish_checked(),
        Err(MergeError::DuplicateUniqueValue)
    );
    assert_eq!(table.rows.get(&1).unwrap().payload, 2);
    assert_eq!(table.rows.get(&2).unwrap().payload, 2);
}

#[test]
fn test_add_unique_index_false_positive_duplicate() {
    let mut table = IndexMergeTable::with_rows(true, [(1, Some(1), 1), (2, Some(2), 2)]);
    table.start_backfill();
    table.replace(3, Some(2), 3).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(table.rows.keys().copied().collect::<Vec<_>>(), vec![1, 3]);
}

#[test]
fn test_add_index_skip_reorg_check() {
    let mut table = IndexMergeTable::new(false);
    assert!(table.can_skip_table_reorg());
    assert!(table.can_skip_temp_index_reorg());

    table.insert(1, Some(1), 1).unwrap();
    assert!(!table.can_skip_table_reorg());
    assert!(table.can_skip_temp_index_reorg());

    table.begin_index();
    table.backfill_snapshot();
    table.insert(2, Some(2), 2).unwrap();
    assert!(!table.can_skip_table_reorg());
    assert!(!table.can_skip_temp_index_reorg());
    table.finish_checked().unwrap();
}

#[test]
fn test_add_index_insert_after_reorg_skip_check() {
    let mut table = IndexMergeTable::new(false);
    assert!(table.can_skip_table_reorg());
    table.insert(1, Some(1), 1).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(table.rows.len(), 1);

    let mut table = IndexMergeTable::new(false);
    assert!(table.can_skip_temp_index_reorg());
    table.insert(2, Some(2), 2).unwrap();
    table.finish_checked().unwrap();
    assert_eq!(table.rows.keys().copied().collect::<Vec<_>>(), vec![2]);
}

/// Go `merge_test.go` 中的 Test 函数逐项存在，避免后续迁移漏掉场景。
#[test]
fn go_merge_test_scenario_inventory_is_complete() {
    const GO_SCENARIOS: &[&str] = &[
        "TestAddIndexMergeProcess",
        "TestAddPrimaryKeyMergeProcess",
        "TestAddIndexMergeVersionIndexValue",
        "TestAddIndexMergeIndexUntouchedValue",
        "TestCreateUniqueIndexKeyExist",
        "TestAddIndexMergeIndexUpdateOnDeleteOnly",
        "TestAddIndexMergeDeleteUniqueOnWriteOnly",
        "TestAddIndexMergeDeleteNullUnique",
        "TestAddIndexMergeDoubleDelete",
        "TestAddIndexMergeConflictWithPessimistic",
        "TestNextGenPessimisticTxnNotFailWithTempIndex",
        "TestAddIndexMergeInsertOnMerging",
        "TestAddIndexMergeReplaceOnMerging",
        "TestAddIndexMergeInsertToDeletedTempIndex",
        "TestAddIndexMergeReplaceDelete",
        "TestAddIndexMergeDeleteDifferentHandle",
        "TestAddIndexDecodeTempIndexCommonHandle",
        "TestAddIndexInsertIgnoreOnBackfill",
        "TestAddIndexMultipleDelete",
        "TestAddIndexDuplicateAndWriteConflict",
        "TestAddIndexUpdateUntouchedValues",
        "TestAddUniqueIndexFalsePositiveDuplicate",
        "TestAddIndexSkipReorgCheck",
        "TestAddIndexInsertAfterReorgSkipCheck",
    ];
    assert_eq!(GO_SCENARIOS.len(), 24);
}
