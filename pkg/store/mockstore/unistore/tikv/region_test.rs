// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

use crate::mock_region::{Peer, Region, RegionEpoch, RegionError, Store};
use crate::region::{RegionManager, RegionOptions, RequestContext, StandAloneRegionManager};

fn manager() -> StandAloneRegionManager {
    StandAloneRegionManager::new(
        Store {
            id: 10,
            address: "store-10".into(),
            labels: Vec::new(),
        },
        Region {
            id: 20,
            start_key: b"a".to_vec(),
            end_key: b"z".to_vec(),
            epoch: RegionEpoch {
                conf_ver: 3,
                version: 4,
            },
            peers: vec![Peer {
                id: 30,
                store_id: 10,
            }],
        },
        RegionOptions::default(),
    )
}

#[test]
fn epoch_must_match_exactly_like_go() {
    let manager = manager();
    let result = manager.get_region_from_context(&RequestContext {
        region_id: 20,
        store_id: Some(10),
        epoch: Some(RegionEpoch {
            conf_ver: 4,
            version: 4,
        }),
    });
    assert!(matches!(result, Err(RegionError::EpochNotMatch(_))));
}

#[test]
fn split_keeps_old_id_on_right_and_preserves_peers_like_go() {
    let manager = manager();
    let regions = manager.split_region(20, vec![b"m".to_vec()]).unwrap();
    assert_eq!(2, regions.len());

    let left = &regions[0];
    assert_ne!(20, left.id);
    assert_eq!(
        RegionEpoch {
            conf_ver: 1,
            version: 1
        },
        left.epoch
    );
    assert_eq!(
        vec![Peer {
            id: 30,
            store_id: 10
        }],
        left.peers
    );

    let right = &regions[1];
    assert_eq!(20, right.id);
    assert_eq!(
        RegionEpoch {
            conf_ver: 3,
            version: 5
        },
        right.epoch
    );
    assert_eq!(
        vec![Peer {
            id: 30,
            store_id: 10
        }],
        right.peers
    );
}
