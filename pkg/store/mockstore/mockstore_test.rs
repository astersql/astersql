// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

use crate::embedded_unistore::{NULL_KEYSPACE_ID as EMBEDDED_NULL_KEYSPACE_ID, New};
use crate::{
    BootstrapWithMultiRegions, MockKeyspaceMeta, MockOptions, NULL_KEYSPACE_ID,
    WithCurrentKeyspaceMeta, WithKeyspacesAndCurrentKeyspaceID,
};

#[test]
fn keyspace_options_match_go_defaults_and_preserve_explicit_gc_mode() {
    let mut options = MockOptions::default();
    assert_eq!(options.current_keyspace_id, NULL_KEYSPACE_ID);
    assert_eq!(options.current_keyspace_meta(), None);

    let mut explicit = MockKeyspaceMeta {
        id: 7,
        name: "tenant".into(),
        ..MockKeyspaceMeta::default()
    };
    explicit
        .config
        .insert("gc_management_type".into(), "unified".into());
    WithCurrentKeyspaceMeta(Some(explicit))(&mut options);
    let current = options.current_keyspace_meta().expect("current keyspace");
    assert_eq!(current.config["gc_management_type"], "unified");

    WithCurrentKeyspaceMeta(None)(&mut options);
    assert_eq!(options.current_keyspace_id, NULL_KEYSPACE_ID);
    assert!(options.cluster_keyspaces.is_empty());

    WithKeyspacesAndCurrentKeyspaceID(
        vec![MockKeyspaceMeta {
            id: 8,
            ..MockKeyspaceMeta::default()
        }],
        8,
    )(&mut options);
    assert_eq!(
        options.current_keyspace_meta().unwrap().config["gc_management_type"],
        "keyspace_level"
    );
}

#[test]
fn multi_region_bootstrap_uses_go_peer_allocation_sequence() {
    let (client, _pd, cluster) =
        New("", Vec::new(), EMBEDDED_NULL_KEYSPACE_ID, Vec::new()).expect("create cluster");
    let (_store, region_ids, peer_ids) =
        BootstrapWithMultiRegions(&cluster, &[b"b".to_vec(), b"d".to_vec()])
            .expect("bootstrap regions");

    let manager = cluster.region_manager();
    let regions = region_ids
        .iter()
        .map(|id| manager.get_region(*id).expect("region"))
        .collect::<Vec<_>>();
    assert_eq!(regions[0].peers[0].id, peer_ids[0]);
    assert_eq!(regions[1].peers[0].id, peer_ids[0]);
    assert_eq!(regions[2].peers[0].id, peer_ids[1]);

    client.close().expect("close client");
}
