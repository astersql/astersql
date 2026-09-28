// Copyright 2026 AsterSQL.

use super::*;
use astersql_lightning_backend_kv::Pairs;
use astersql_lightning_verification::KvPair;

#[test]
fn encoded_batch_preserves_source_row_count_independently_of_record_kv_count() {
    let pairs = Pairs {
        Pairs: vec![
            KvPair {
                key: b"t1_r1".to_vec(),
                val: b"first".to_vec(),
            },
            KvPair {
                key: b"t1_r2".to_vec(),
                val: b"second".to_vec(),
            },
        ],
        ..Pairs::default()
    };
    let mut batch = NewEncodedKVGroupBatch(&[], 1);
    batch.Add(&pairs).unwrap();

    assert_eq!(1, batch.row_count);
    assert_eq!(2, batch.data_kvs.len());
}
