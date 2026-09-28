// Copyright 2026 AsterSQL.

use crate::mock::New;
use crate::pd::NULL_KEYSPACE_ID;
use crate::rpc::{Request, Response};
use crate::tikv::mvcc::{Mutation, MutationOp, PrewriteRequest};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn debug_region_properties_reports_mvcc_row_count_like_go() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after the Unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("tidb-unistore-temp-rpc-{nonce}"));
    let (client, _pd, cluster) =
        New(&path, Vec::new(), NULL_KEYSPACE_ID, Vec::new()).expect("create unistore");
    let region_id = cluster.region_manager().scan_regions(b"", b"", 1)[0]
        .meta
        .id;
    let address = cluster.region_manager().all_stores()[0].address.clone();

    let store = client.mvcc_store();
    for (index, key) in [b"a".as_slice(), b"b".as_slice()].into_iter().enumerate() {
        let start_ts = index as u64 + 1;
        store
            .prewrite(&PrewriteRequest {
                mutations: vec![Mutation {
                    op: MutationOp::Put,
                    key: key.to_vec(),
                    value: key.to_vec(),
                    is_pessimistic_lock: false,
                }],
                primary_lock: key.to_vec(),
                start_ts,
                lock_ttl: 100,
                ..PrewriteRequest::default()
            })
            .expect("prewrite row");
        store
            .commit(&[key.to_vec()], start_ts, start_ts + 10)
            .expect("commit row");
    }

    let response = client
        .send_request(
            &address,
            Request::DebugGetRegionProperties { region_id },
            Duration::from_secs(2),
        )
        .expect("get region properties");
    match response {
        Response::RegionProperties(properties) => assert_eq!(
            properties,
            vec![("mvcc.num_rows".to_owned(), "2".to_owned())]
        ),
        _ => panic!("unexpected response type"),
    }

    client.close().expect("close unistore");
}
