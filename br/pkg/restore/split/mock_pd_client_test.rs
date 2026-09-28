// Copyright 2026 AsterSQL.

use std::collections::HashMap;

use super::*;
use crate::stubs::{Context, codec, metapb};

#[test]
fn test_test_client_split_wait_and_scatter_matches_go() {
    let original = RegionInfo {
        Region: Some(metapb::Region {
            Id: 7,
            StartKey: Vec::new(),
            EndKey: Vec::new(),
            Peers: vec![metapb::Peer { Id: 11, StoreId: 1 }],
            ..Default::default()
        }),
        ..Default::default()
    };
    let client = NewTestClient(HashMap::new(), HashMap::from([(7, original)]), 8);

    let split_key = b"middle".to_vec();
    let created = client
        .SplitWaitAndScatter(
            &Context::default(),
            &RegionInfo::default(),
            &[split_key.clone()],
        )
        .expect("Go TestClient splits an interior encoded key");

    assert_eq!(created.len(), 1);
    let encoded = codec::EncodeBytes(Vec::new(), &split_key);
    let new_meta = created[0].Region.as_ref().unwrap();
    assert_eq!(new_meta.Id, 8);
    assert_eq!(new_meta.StartKey, Vec::<u8>::new());
    assert_eq!(new_meta.EndKey, encoded);
    assert_eq!(new_meta.Peers[0].Id, 11);

    let regions = client.GetAllRegions();
    assert_eq!(regions.len(), 2);
    assert_eq!(regions[&7].Region.as_ref().unwrap().StartKey, encoded);
    assert_eq!(regions[&8].Region.as_ref().unwrap().EndKey, encoded);
}

#[test]
fn test_mock_pd_set_regions_appends_and_split_accepts_empty_key() {
    let client = NewMockPDClientForSplit();
    client.SetRegions(&[b"a".to_vec(), b"z".to_vec()]);
    client.SetRegions(&[b"z".to_vec(), Vec::new()]);
    assert_eq!(client.scan_regions_tree().regions.len(), 2);

    let region = client.GetRegion(b"a").unwrap();
    let (_, created) = client.SplitRegion(&region, &[Vec::new()], false).unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(
        created[0].Region.as_ref().unwrap().StartKey,
        codec::EncodeBytes(Vec::new(), &[])
    );
}

#[test]
fn test_fake_pd_http_get_placement_rule_creates_missing_rule() {
    let client = NewFakePDHTTPClient();
    let rule = client.GetPlacementRule("group", "rule").unwrap();
    assert_eq!(rule.GroupID, "group");
    assert_eq!(rule.ID, "rule");
    assert_eq!(
        client.GetPlacementRule("other", "rule").unwrap().GroupID,
        "group"
    );
}

#[test]
fn test_fake_split_client_empty_end_matches_go_comparison() {
    let client = NewFakeSplitClient();
    client.AppendRegion(b"a".to_vec(), b"z".to_vec());
    assert!(
        client
            .ScanRegions(&Context::default(), b"a", b"", 10, &[])
            .unwrap()
            .is_empty()
    );
}
