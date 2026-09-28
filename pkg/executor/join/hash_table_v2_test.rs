// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Hash Join V2 哈希表容量、构建、lookup 与行迭代器的单元测试。
//
// 校验 `next_power_of_two` / 最小桶长边界、分区 build/lookup/replace/clear，
// 以及跨空分区与非空分区的 `RowIter` 边界行为。


use crate::hash_table_v2::{
    HashTableV2, SubTable, get_hash_table_length_by_row_len, get_hash_table_memory_usage,
    next_power_of_two,
};
use crate::join_row_table::{RowTable, RowTableSegment};
use crate::join_table_meta::EncodedRow;
use std::sync::atomic::AtomicBool;

/// 构造只含指定 hash 值与有效 key 数的单 segment `RowTable` 测试夹具。
fn table(hashes: &[u64], valid: usize) -> RowTable {
    let rows = hashes
        .iter()
        .map(|hash| EncodedRow {
            bytes: hash.to_le_bytes().to_vec(),
            null_map: vec![0],
            key_offset: 0,
            key_length: 8,
            row_data_offset: 8,
            used: AtomicBool::new(false),
        })
        .collect();
    let mut table = RowTable::default();
    table.segments_mut().push(RowTableSegment {
        rows,
        hash_values: hashes.to_vec(),
        valid_key_count: valid as u64,
        ..Default::default()
    });
    table
}

/// 对齐 Go 边界：2 的幂上取整、最小桶长 32、内存按 usize 宽度估算。
#[test]
fn hash_table_size_rounding_and_memory_match_go_boundaries() {
    assert_eq!(next_power_of_two(0), 2);
    assert_eq!(next_power_of_two(1), 2);
    assert_eq!(next_power_of_two(2), 4);
    assert_eq!(next_power_of_two(31), 32);
    assert_eq!(get_hash_table_length_by_row_len(0), 32);
    assert_eq!(get_hash_table_length_by_row_len(32), 64);
    assert_eq!(
        get_hash_table_memory_usage(64),
        64 * std::mem::size_of::<usize>() as i64
    );
}

/// 覆盖 build 后的 lookup、清空分区、替换分区以及越界错误路径。
#[test]
fn hash_table_v2_build_lookup_partition_replace_and_clear_are_real() {
    let mut hash_table = HashTableV2::new(vec![table(&[11, 22, 11], 3), table(&[33], 1)]);
    assert_eq!(hash_table.total_row_count(), 4);
    // 同一 hash 11 应命中两条冲突行。
    assert_eq!(hash_table.lookup(0, 11).len(), 2);
    assert_eq!(hash_table.lookup(0, 99).len(), 0);
    assert!(hash_table.total_memory_usage() > 0);
    hash_table.clear_partition_segments(0);
    assert_eq!(hash_table.total_row_count(), 1);
    hash_table
        .replace_partition(0, table(&[44, 44], 2))
        .unwrap();
    assert_eq!(hash_table.lookup(0, 44).len(), 2);
    assert!(hash_table.replace_partition(2, table(&[], 0)).is_err());
}

/// 行迭代器应跳过空分区，并在 total 边界处给出哨兵 `RowPos`。
#[test]
fn row_iterator_crosses_empty_and_nonempty_partitions_at_exact_boundaries() {
    let hash_table = HashTableV2::new(vec![table(&[], 0), table(&[1, 2], 2), table(&[3], 1)]);
    let positions: Vec<_> = hash_table.create_row_iter(0, 3).unwrap().collect();
    assert_eq!(positions.len(), 3);
    assert_eq!(positions[0].sub_table_index, 1);
    assert_eq!(positions[2].sub_table_index, 2);
    assert_eq!(
        hash_table.get_row(positions[2]).unwrap().bytes,
        3_u64.to_le_bytes()
    );
    // create_row_pos(total) 为结束哨兵；超过则报错。
    assert_eq!(hash_table.create_row_pos(3).unwrap().sub_table_index, 3);
    assert!(hash_table.create_row_pos(4).is_err());
}

#[test]
/// Disjoint row iterator ranges cover every row exactly once when split at a boundary.
fn row_iter_split_ranges_cover_all_rows_once() {
    let hash_table = HashTableV2::new(vec![table(&[7, 8], 2), table(&[9, 10], 2)]);
    let left: Vec<_> = hash_table.create_row_iter(0, 2).unwrap().collect();
    let right: Vec<_> = hash_table.create_row_iter(2, 4).unwrap().collect();
    assert_eq!(left.len() + right.len(), 4);
    assert!(left.iter().all(|position| position.sub_table_index == 0));
    assert!(right.iter().all(|position| position.sub_table_index == 1));
    assert!(left.iter().all(|position| !right.contains(position)));
}

#[test]
/// Clearing a partition removes its rows and memory while preserving other partitions.
fn clearing_one_hash_table_partition_preserves_other_partition_rows() {
    let mut hash_table = HashTableV2::new(vec![table(&[1, 2], 2), table(&[3, 4], 2)]);
    let memory_before = hash_table.total_memory_usage();
    hash_table.clear_partition_segments(1);
    assert_eq!(hash_table.total_row_count(), 2);
    assert_eq!(hash_table.lookup(0, 1).len(), 1);
    assert!(hash_table.lookup(1, 3).is_empty());
    assert!(hash_table.total_memory_usage() < memory_before);
}

/// Go builds buckets from `validJoinKeyPos`, not from the first N rows.
#[test]
fn build_indexes_exact_valid_join_key_positions() {
    let rows = [10_u64, 20, 30]
        .into_iter()
        .map(|hash| EncodedRow {
            bytes: hash.to_le_bytes().to_vec(),
            null_map: vec![0],
            key_offset: 0,
            key_length: 8,
            row_data_offset: 8,
            used: AtomicBool::new(false),
        })
        .collect();
    let mut row_table = RowTable::default();
    row_table.segments_mut().push(RowTableSegment {
        rows,
        hash_values: vec![10, 20, 30],
        valid_join_key_positions: vec![2],
        valid_key_count: 1,
        ..Default::default()
    });

    let hash_table = HashTableV2::new(vec![row_table]);
    assert!(hash_table.lookup(0, 10).is_empty());
    assert_eq!(hash_table.lookup(0, 30).len(), 1);
    assert_eq!(hash_table.lookup(0, 30)[0].row_index, 2);
}

/// Go clears storage during spill without rewriting the cached empty flags.
#[test]
fn clear_segments_preserves_go_empty_flags_and_memory_formula() {
    let row_table = table(&[1, 2], 2);
    let row_memory = row_table.total_memory_usage();
    let mut sub_table = SubTable::new(row_table, 0);
    assert_eq!(
        sub_table.total_memory_usage(),
        row_memory + get_hash_table_memory_usage(32)
    );
    assert!(!sub_table.is_row_table_empty);
    assert!(!sub_table.is_hash_table_empty);

    sub_table.clear_segments();
    assert!(!sub_table.is_row_table_empty);
    assert!(!sub_table.is_hash_table_empty);
    assert_eq!(sub_table.total_memory_usage(), 0);
}
