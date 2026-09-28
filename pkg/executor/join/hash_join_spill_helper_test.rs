// Copyright 2026 AsterSQL.

use crate::hash_join_spill_helper::HashJoinSpillHelper;
use crate::join_row_table::{RowTable, RowTableSegment};
use crate::join_table_meta::EncodedRow;
use std::sync::atomic::AtomicBool;

fn table(hash: u64) -> RowTable {
    let mut table = RowTable::default();
    table.segments_mut().push(RowTableSegment {
        rows: vec![EncodedRow {
            bytes: vec![hash as u8; 8],
            null_map: vec![0],
            key_offset: 0,
            key_length: 8,
            row_data_offset: 8,
            used: AtomicBool::new(false),
        }],
        hash_values: vec![hash],
        valid_key_count: 1,
        ..Default::default()
    });
    table
}

#[test]
fn spill_remaining_rows_only_spills_partitions_already_marked_by_go_path() {
    let helper = HashJoinSpillHelper::new(2, 1, 2, 100).unwrap();
    helper.memory_tracker.consume(1_000);
    helper.set_partition_spilled(&[0]).unwrap();
    let mut worker_tables = vec![vec![table(1), table(2)]];

    helper
        .spill_remaining_rows(&mut worker_tables, &[10, 100])
        .unwrap();

    assert_eq!(helper.spilled_partitions(), vec![0]);
    assert!(worker_tables[0][0].segments().is_empty());
    assert_eq!(worker_tables[0][1].segments().len(), 1);
}
