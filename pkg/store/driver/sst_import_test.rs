// Copyright 2026 AsterSQL.

use super::*;
use astersql_kv as kv;

#[test]
fn invalid_input_is_rejected_before_connecting() {
    assert!(write_and_ingest(&[], None, 0, vec![]).is_err());
    assert!(
        write_and_ingest(
            &[],
            None,
            1,
            vec![(b"b".to_vec(), vec![]), (b"a".to_vec(), vec![])]
        )
        .is_err()
    );
    assert!(
        write_and_ingest(
            &[],
            None,
            1,
            vec![(b"a".to_vec(), vec![]), (b"a".to_vec(), vec![])]
        )
        .is_err()
    );
    assert_eq!(
        write_and_ingest(&[], None, 1, vec![]).unwrap().write_rpcs,
        0
    );
}

#[test]
fn pd_keys_match_memcomparable_boundary_encoding() {
    assert_eq!(region_key(b""), vec![0, 0, 0, 0, 0, 0, 0, 0, 247]);
    assert_eq!(
        region_key(b"12345678"),
        [b"12345678".as_slice(), &[255, 0, 0, 0, 0, 0, 0, 0, 0, 247]].concat()
    );
    assert!(region_key(b"12345678") < region_key(b"12345678\0"));
}

#[test]
#[ignore = "requires REAL_TIKV_PD three-node cluster"]
fn real_sst_write_and_multi_ingest_preserve_mvcc_visibility() {
    let pd = std::env::var("REAL_TIKV_PD").expect("REAL_TIKV_PD required");
    let mut driver = crate::TiKVDriver::default();
    let store = driver.Open(&format!("tikv://{pd}?disableGC=true")).unwrap();
    let old_version = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope).unwrap();
    let prefix = format!("astersql/sst-import/{}/", old_version.Ver);
    let keys = [
        format!("{prefix}a").into_bytes(),
        format!("{prefix}b").into_bytes(),
    ];
    let old = kv::Storage::GetSnapshot(&store, old_version);
    let commit_ts = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
        .unwrap()
        .Ver;
    let stats = write_and_ingest(
        &store.GetPDAddrs().unwrap(),
        store.TLSConfig(),
        commit_ts,
        vec![
            (keys[0].clone(), b"first".to_vec()),
            (keys[1].clone(), b"second".to_vec()),
        ],
    )
    .unwrap();
    assert_eq!(stats.keys, 2);
    assert!(
        stats.write_rpcs >= 3,
        "all three replicas must receive SST writes: {stats:?}"
    );
    assert!(stats.ingest_rpcs >= 1);
    let context = kv::Context::todo();
    assert!(old.Get(&context, kv::Key(keys[0].clone()), &[]).is_err());
    let current = kv::Storage::GetSnapshot(&store, kv::MaxVersion);
    assert_eq!(
        current
            .Get(&context, kv::Key(keys[0].clone()), &[])
            .unwrap()
            .Value,
        b"first"
    );
    assert_eq!(
        current
            .Get(&context, kv::Key(keys[1].clone()), &[])
            .unwrap()
            .Value,
        b"second"
    );
    let mut cleanup = kv::Storage::Begin(&store, &[]).unwrap();
    for key in keys {
        cleanup.Delete(kv::Key(key)).unwrap();
    }
    cleanup.Commit(&context).unwrap();
    println!("physical import {stats:?}, commit_ts={commit_ts}");
}
