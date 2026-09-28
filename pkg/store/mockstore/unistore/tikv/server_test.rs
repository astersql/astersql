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
