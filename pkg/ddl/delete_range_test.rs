// Copyright 2026 AsterSQL.

use crate::delete_range::{DeleteRangeAction, DeleteRangeJob, IndexArgument, add_delete_range_job};

fn job(action: DeleteRangeAction) -> DeleteRangeJob {
    DeleteRangeJob {
        id: 7,
        table_id: 42,
        action,
        rollback_done: false,
        cancelled: false,
        old_physical_table_ids: Vec::new(),
        partition_ids: Vec::new(),
        index_arguments: Vec::new(),
        index_ids: Vec::new(),
        old_global_indexes: Vec::new(),
        subjobs: Vec::new(),
    }
}

fn table_prefix(table_id: i64) -> Vec<u8> {
    let mut key = b"t".to_vec();
    key.extend_from_slice(&((table_id as u64) ^ (1_u64 << 63)).to_be_bytes());
    key
}

fn index_prefix(table_id: i64, index_id: i64) -> Vec<u8> {
    let mut key = table_prefix(table_id);
    key.extend_from_slice(b"_i");
    key.extend_from_slice(&((index_id as u64) ^ (1_u64 << 63)).to_be_bytes());
    key
}

#[test]
fn range_end_keys_match_go_signed_integer_wraparound() {
    let mut table_job = job(DeleteRangeAction::DropSchema);
    table_job.old_physical_table_ids = vec![i64::MAX];
    let table_tasks = add_delete_range_job(&table_job);
    assert_eq!(table_tasks[0].end_key, table_prefix(i64::MIN));

    let mut index_job = job(DeleteRangeAction::DropColumn);
    index_job.table_id = i64::MAX;
    index_job.index_ids = vec![i64::MAX];
    let index_tasks = add_delete_range_job(&index_job);
    assert_eq!(index_tasks[0].end_key, index_prefix(i64::MAX, i64::MIN));
}

#[test]
fn multi_schema_subjobs_use_parent_job_identity_like_go_proxy_jobs() {
    let mut parent = job(DeleteRangeAction::MultiSchemaChange);
    let mut subjob = job(DeleteRangeAction::DropTable);
    subjob.id = 99;
    subjob.table_id = 999;
    parent.subjobs.push(subjob);

    let tasks = add_delete_range_job(&parent);

    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].job_id, parent.id);
    assert_eq!(tasks[0].start_key, table_prefix(parent.table_id));
}

#[test]
fn dropped_columnar_index_is_filtered_before_range_generation() {
    let mut drop_index = job(DeleteRangeAction::DropIndex);
    drop_index.index_arguments.push(IndexArgument {
        index_id: 11,
        global: false,
        columnar: true,
        table_id: drop_index.table_id,
    });

    assert!(add_delete_range_job(&drop_index).is_empty());
}
