// Copyright 2026 AsterSQL.

use std::collections::BTreeMap;

use super::index_merge_tmp::{
    MergeError, OriginalIndexValue, TemporaryIndexBuffers, TemporaryIndexRecord,
    batch_check_temporary_unique_key, check_temporary_index_key, fetch_temporary_index_values,
    find_index_info_by_decoding_key,
};

fn record(
    key: &[u8],
    value: &[u8],
    handle: &[u8],
    delete: bool,
    distinct: bool,
) -> TemporaryIndexRecord {
    TemporaryIndexRecord {
        temporary_key: key.to_vec(),
        original_key: b"origin".to_vec(),
        value: value.to_vec(),
        handle: handle.to_vec(),
        delete,
        distinct,
        skip: false,
    }
}

#[test]
fn check_key_matches_go_insert_and_delete_branches() {
    let mut insert = record(b"tmp", b"h1", b"h1", false, true);
    assert!(
        !check_temporary_index_key(&mut insert, &OriginalIndexValue::distinct(b"h1", true),)
            .unwrap()
    );
    assert!(insert.skip);

    let mut duplicate = record(b"tmp", b"h2", b"h2", false, true);
    assert_eq!(
        check_temporary_index_key(&mut duplicate, &OriginalIndexValue::distinct(b"h1", true),),
        Err(MergeError::DuplicateKey)
    );

    let mut matching_delete = record(b"tmp", b"", b"h1", true, true);
    assert!(
        check_temporary_index_key(
            &mut matching_delete,
            &OriginalIndexValue::distinct(b"h1", true),
        )
        .unwrap()
    );
    assert!(!matching_delete.skip);

    let mut live_conflict = record(b"tmp", b"", b"h2", true, true);
    assert!(
        !check_temporary_index_key(
            &mut live_conflict,
            &OriginalIndexValue::distinct(b"h1", true),
        )
        .unwrap()
    );
    assert!(live_conflict.skip);

    let mut deleted_conflict = record(b"tmp", b"", b"h2", true, true);
    assert!(
        !check_temporary_index_key(
            &mut deleted_conflict,
            &OriginalIndexValue::distinct(b"h1", false),
        )
        .unwrap()
    );
    assert!(!deleted_conflict.skip);

    let mut lookup_error = record(b"tmp", b"", b"h2", true, true);
    assert_eq!(
        check_temporary_index_key(
            &mut lookup_error,
            &OriginalIndexValue::distinct_lookup_error(b"h1", MergeError::Decode),
        ),
        Err(MergeError::Decode)
    );

    let mut non_distinct_delete = record(b"tmp", b"", b"h2", true, true);
    assert!(
        !check_temporary_index_key(
            &mut non_distinct_delete,
            &OriginalIndexValue::non_distinct(b"encoded"),
        )
        .unwrap()
    );
    assert!(!non_distinct_delete.skip);
}

#[test]
fn batch_check_matches_go_unique_and_in_batch_duplicate_rules() {
    let mut records = vec![
        record(b"t1", b"h1", b"h1", false, true),
        record(b"t2", b"h2", b"h2", false, true),
    ];
    assert_eq!(
        batch_check_temporary_unique_key(&mut records, &BTreeMap::new(), true),
        Err(MergeError::DuplicateKey)
    );

    let mut non_unique = vec![record(b"t1", b"h1", b"h1", false, true)];
    batch_check_temporary_unique_key(&mut non_unique, &BTreeMap::new(), false).unwrap();
    assert!(!non_unique[0].skip);
}

#[test]
fn fetch_uses_storage_order_and_only_empty_scan_finishes() {
    let records = vec![
        record(b"c", b"3", b"3", false, true),
        record(b"a", b"1", b"1", false, true),
        record(b"b", b"2", b"2", false, true),
    ];
    let mut buffers = TemporaryIndexBuffers::with_capacity(2);
    let first = fetch_temporary_index_values(&records, b"a", b"z", 2, &mut buffers).unwrap();
    assert_eq!(buffers.temporary_keys, [b"a".to_vec(), b"b".to_vec()]);
    assert_eq!(first.next_key, b"b\0");
    assert!(!first.done);
    assert_eq!(first.add_count, 0);

    let short = fetch_temporary_index_values(&records, b"c", b"z", 2, &mut buffers).unwrap();
    assert_eq!(short.scan_count, 1);
    assert!(!short.done);
    let empty =
        fetch_temporary_index_values(&records, &short.next_key, b"z", 2, &mut buffers).unwrap();
    assert!(empty.done);
    assert_eq!(empty.next_key, b"z\0");
}

#[test]
fn index_id_is_decoded_from_tablecodec_header_and_masked() {
    const SIGN_MASK: u64 = 1_u64 << 63;
    let table_id = 42_i64;
    let index_id = 7_i64;
    let temporary_prefix = 0x7fff_0000_0000_0000_i64;
    let mut key = vec![b't'];
    key.extend_from_slice(&((table_id as u64) ^ SIGN_MASK).to_be_bytes());
    key.extend_from_slice(b"_i");
    key.extend_from_slice(&(((index_id | temporary_prefix) as u64) ^ SIGN_MASK).to_be_bytes());
    key.extend_from_slice(b"suffix-that-must-not-be-decoded");

    assert_eq!(
        find_index_info_by_decoding_key(&[index_id], &key),
        Ok(index_id)
    );
    assert_eq!(
        find_index_info_by_decoding_key(&[8], &key),
        Err(MergeError::Decode)
    );
}
