// Copyright 2026 AsterSQL.

use super::split_region::{
    SplitError, SplitTableInfo, encode_record_key, normalize_split_policy, pre_split_record_keys,
    wait_scatter_finished,
};
use std::cell::Cell;

#[test]
fn normalize_policy_accepts_one_region_like_go() {
    let policy = normalize_split_policy(
        vec!["0".to_owned()],
        vec!["10".to_owned()],
        1,
        Vec::new(),
        None,
    )
    .expect("Go accepts every positive region count");

    assert_eq!(policy.num, 1);
}

#[test]
fn normalize_policy_does_not_compare_expression_text_lexically() {
    normalize_split_policy(
        vec!["10".to_owned()],
        vec!["2".to_owned()],
        2,
        Vec::new(),
        None,
    )
    .expect("Go validates expressions here, not their restored string ordering");
}

#[test]
fn shard_pre_split_uses_table_prefix_and_non_negative_handle_space() {
    let info = SplitTableInfo {
        table_id: 42,
        partition_ids: Vec::new(),
        shard_row_id_bits: 4,
        pre_split_regions: 2,
        index_ids: Vec::new(),
    };

    let keys = pre_split_record_keys(&info).expect("valid shard split settings");
    let mut table_prefix = b"t".to_vec();
    table_prefix.extend_from_slice(&42_i64.to_be_bytes());

    assert_eq!(
        keys,
        vec![
            table_prefix,
            encode_record_key(42, 1_i64 << 61),
            encode_record_key(42, 1_i64 << 62),
            encode_record_key(42, 3_i64 << 61),
        ]
    );
}

#[test]
fn shard_pre_split_preserves_physical_table_order_like_go() {
    let info = SplitTableInfo {
        table_id: 42,
        partition_ids: vec![9, 3],
        shard_row_id_bits: 1,
        pre_split_regions: 0,
        index_ids: Vec::new(),
    };

    let keys = pre_split_record_keys(&info).expect("valid shard split settings");
    assert_eq!(&keys[0][1..9], &9_i64.to_be_bytes());
    assert_eq!(&keys[1][1..9], &3_i64.to_be_bytes());
}

#[test]
fn scatter_wait_stops_after_the_first_non_pd_error_like_go() {
    let visited = Cell::new(0_u64);
    let results = [Ok(()), Err(()), Ok(())].into_iter().map(|result| {
        visited.set(visited.get() + 1);
        result
    });

    assert_eq!(
        wait_scatter_finished(results),
        Err(SplitError::ScatterFailed(1))
    );
    assert_eq!(visited.get(), 2);
}
