// Copyright 2026 AsterSQL.

use serde_json::json;

use super::*;

#[test]
fn group_settings_path_prefix_matches_pd_client() {
    assert_eq!(
        group_settings_path_prefix(u32::MAX),
        b"resource_group/settings"
    );
    assert_eq!(
        group_settings_path_prefix(1),
        b"resource_group/keyspace/settings/1"
    );
}

#[test]
fn pd_http_wire_names_match_go_types() {
    let params = UpdateKeyspaceConfigParams {
        Config: [("gc_management_type".into(), Some("global_gc".into()))]
            .into_iter()
            .collect(),
        Preconditions: Default::default(),
    };
    assert_eq!(
        serde_json::to_value(params).unwrap(),
        json!({"config": {"gc_management_type": "global_gc"}})
    );

    let stores = StoresInfo {
        Count: 1,
        Stores: vec![StoreInfo {
            Store: StoreMeta {
                ID: 7,
                Address: "store:20160".into(),
                StatusAddress: "store:20180".into(),
                StateName: "Up".into(),
                Labels: [("engine".into(), "tiflash".into())].into_iter().collect(),
            },
        }],
    };
    assert_eq!(
        serde_json::to_value(stores).unwrap(),
        json!({
            "count": 1,
            "stores": [{
                "store": {
                    "id": 7,
                    "address": "store:20160",
                    "status_address": "store:20180",
                    "state_name": "Up",
                    "labels": {"engine": "tiflash"}
                }
            }]
        })
    );

    let patch = LabelRulePatch {
        DeleteRules: vec!["obsolete".into()],
        SetRules: Vec::new(),
    };
    assert_eq!(
        serde_json::to_value(patch).unwrap(),
        json!({"sets": [], "deletes": ["obsolete"]})
    );

    let region_stats: RegionDistributions = serde_json::from_value(json!({
        "count": 2,
        "store_peer_count": {"7": 2}
    }))
    .unwrap();
    assert_eq!(region_stats.RegionCount, 2);
    assert_eq!(region_stats.StorePeerCount.get(&7), Some(&2));
}
