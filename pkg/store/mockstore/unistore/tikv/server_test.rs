// Copyright 2026 AsterSQL.

use crate::inner_server::{DatabaseBundle, StandAloneInnerServer};
use crate::mock_region::{Peer, Region, RegionEpoch, Store};
use crate::mvcc::{MvccStore, SafePoint};
use crate::region::{RegionOptions, StandAloneRegionManager};
use crate::server::Server;
use std::sync::Arc;

#[derive(Default)]
struct TestBundle;

impl DatabaseBundle for TestBundle {
    fn close(&self) -> Result<(), String> {
        Ok(())
    }
}

fn test_server() -> Server {
    let region_manager = Arc::new(StandAloneRegionManager::new(
        Store {
            id: 1,
            address: "store-1".into(),
            labels: Vec::new(),
        },
        Region {
            id: 1,
            start_key: Vec::new(),
            end_key: Vec::new(),
            epoch: RegionEpoch {
                conf_ver: 1,
                version: 1,
            },
            peers: vec![Peer { id: 1, store_id: 1 }],
        },
        RegionOptions {
            store_address: "store-1".into(),
            pd_address: "pd-1".into(),
            region_size: 1024,
        },
    ));
    let bundle = Arc::new(TestBundle);
    let inner = Arc::new(StandAloneInnerServer::new(bundle));
    let store = Arc::new(MvccStore::new(Arc::new(SafePoint::new(0))));
    Server::new(region_manager, store, inner)
}

#[test]
fn raw_get_key_ttl_hides_expired_keys() {
    let server = test_server();
    server.raw_put(b"expired".to_vec(), b"value".to_vec(), Some(5), 100);

    assert_eq!(None, server.raw_get_key_ttl(b"expired", 105));
}

#[test]
fn injected_pessimistic_deadlock_uses_first_mutation_before_region_validation() {
    let _scenario = fail::FailScenario::setup();
    fail::cfg(
        "pessimisticLockReturnDeadlock",
        "1*return(true)->return(false)",
    )
    .unwrap();
    let server = test_server();
    let request = crate::mvcc::PessimisticLockRequest {
        mutations: vec![crate::mvcc::Mutation {
            key: b"first".to_vec(),
            op: crate::mvcc::MutationOp::PessimisticLock,
            value: Vec::new(),
            is_pessimistic_lock: true,
        }],
        start_ts: 41,
        for_update_ts: 43,
        ..Default::default()
    };
    let response = server.kv_pessimistic_lock(&Default::default(), &request);
    assert!(
        response.key_error.is_some(),
        "first request must return the injected deadlock"
    );
    let error = response.key_error.unwrap();
    assert_eq!(
        error.deadlock,
        Some(crate::mvcc::MvccError::Deadlock {
            lock_key: b"first".to_vec(),
            lock_ts: 42,
            deadlock_key_hash: crate::util::keys_to_hash_values(&[b"first".to_vec()])[0],
        })
    );
    assert!(!error.retryable && !error.abort);
    assert!(response.region_error.is_none());
    // The second evaluation is false and proceeds to normal region validation.
    assert!(
        server
            .kv_pessimistic_lock(&Default::default(), &request)
            .region_error
            .is_some()
    );
    fail::cfg("pessimisticLockReturnDeadlock", "return(true)").unwrap();
    let empty = crate::mvcc::PessimisticLockRequest::default();
    assert!(
        server
            .kv_pessimistic_lock(&Default::default(), &empty)
            .key_error
            .is_none()
    );
    fail::remove("pessimisticLockReturnDeadlock");
}

#[test]
fn port_74dc8d6e87_mock_point_read_exec_details() {
    use crate::mvcc::{KvPair, MvccError};
    use crate::server::mock_point_read_exec_details;
    for (pairs, keys, bytes) in [
        (vec![], 0, 0),
        (
            vec![KvPair {
                key: b"missing".to_vec(),
                value: vec![],
                error: None,
            }],
            0,
            0,
        ),
        (
            vec![
                KvPair {
                    key: b"bad".to_vec(),
                    value: b"bad".to_vec(),
                    error: Some(MvccError::InvalidRequest("bad".into())),
                },
                KvPair {
                    key: b"k1".to_vec(),
                    value: b"abc".to_vec(),
                    error: None,
                },
                KvPair {
                    key: b"k2".to_vec(),
                    value: b"abcdef".to_vec(),
                    error: None,
                },
            ],
            2,
            13,
        ),
    ] {
        let detail = mock_point_read_exec_details(&pairs);
        if astersql_config_kerneltype::IsNextGen() {
            let detail = detail.unwrap();
            assert_eq!(
                (
                    detail.total_versions,
                    detail.processed_versions,
                    detail.total_versions_size,
                    detail.processed_versions_size
                ),
                (keys, keys, bytes, bytes)
            );
        } else {
            assert!(detail.is_none());
        }
    }
}

#[test]
fn port_74dc8d6e87_get_batch_and_buffer_return_scan_details() {
    use crate::mvcc::{Mutation, MutationOp, PrewriteRequest};
    use crate::server::RpcContext;
    let server = test_server();
    let context = RpcContext {
        region: crate::region::RequestContext {
            region_id: 1,
            store_id: Some(1),
            epoch: Some(RegionEpoch {
                conf_ver: 1,
                version: 1,
            }),
        },
        ..Default::default()
    };
    let keys = vec![b"k1".to_vec(), b"k2".to_vec()];
    let response = server.kv_prewrite(
        &context,
        &PrewriteRequest {
            mutations: keys
                .iter()
                .zip([b"abc".as_slice(), b"abcdef".as_slice()])
                .map(|(key, value)| Mutation {
                    key: key.clone(),
                    value: value.to_vec(),
                    op: MutationOp::Put,
                    is_pessimistic_lock: false,
                })
                .collect(),
            primary_lock: keys[0].clone(),
            start_ts: 1,
            lock_ttl: 100,
            ..Default::default()
        },
    );
    assert!(response.key_error.is_none() && response.region_error.is_none());
    let buffered = server.kv_buffer_batch_get(&context, &keys, 1);
    assert_eq!(buffered.value.unwrap().len(), 2);
    let committed = server.kv_commit(&context, &keys, 1, 2);
    assert!(committed.key_error.is_none() && committed.region_error.is_none());
    let hit = server.kv_get(&context, keys[0].clone(), 10);
    assert_eq!(hit.value.unwrap(), Some(b"abc".to_vec()));
    let batch = server.kv_batch_get(
        &context,
        &[keys[0].clone(), keys[1].clone(), b"missing".to_vec()],
        10,
    );
    assert_eq!(batch.value.unwrap().len(), 2);
    let miss = server.kv_get(&context, b"missing".to_vec(), 10);
    assert_eq!(miss.value.unwrap(), None);
    for (detail, expected) in [
        (buffered.scan_detail, 13),
        (hit.scan_detail, 5),
        (batch.scan_detail, 13),
        (miss.scan_detail, 0),
    ] {
        if astersql_config_kerneltype::IsNextGen() {
            assert_eq!(detail.unwrap().total_versions_size, expected);
        } else {
            assert!(detail.is_none());
        }
    }
}
