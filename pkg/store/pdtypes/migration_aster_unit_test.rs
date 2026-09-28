// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// AsterSQL 迁移测试：校验 StringSlice、RegionTree、配置/放置规则与统计的 JSON 契约。

use crate::api::{
    MetaPeer, MetaStore, PDPeerStats, RegionInfo, RegionsInfo, ReplicationStatus, StoreInfo,
    StoreStatus, StoresInfo,
};
use crate::config::ReplicationConfig;
use crate::configtypes::Duration;
use crate::placement::{LabelConstraint, LabelConstraintOp, PeerRoleType, Rule, Voter};
use crate::region_tree::{NewRegionInfo, RegionTree};
use crate::statistics::RegionStats;
use crate::typeutil::StringSlice;
use kvproto::{metapb, pdpb};
use std::collections::HashMap;

/// API DTO 与 Go 一致：公开结构可按显式 json tag 往返编码。
#[test]
fn migration_api_types_keep_go_json_contract() {
    let stores = StoresInfo::default();
    assert_eq!(
        serde_json::to_value(&stores).unwrap(),
        serde_json::json!({"count": 0, "stores": null})
    );
    assert_eq!(
        serde_json::to_value(RegionsInfo::default()).unwrap(),
        serde_json::json!({"count": 0, "regions": null})
    );
    assert_eq!(
        serde_json::from_value::<StoresInfo>(serde_json::json!({
            "count": 1,
            "stores": []
        }))
        .unwrap()
        .Count,
        1
    );

    let replication = ReplicationStatus {
        State: "simple-majority".into(),
        StateID: 42,
    };
    assert_eq!(
        serde_json::to_value(replication).unwrap(),
        serde_json::json!({
            "state": "simple-majority",
            "state_id": 42
        })
    );
}

/// Go encoding/json 对缺失字段保留结构零值。
#[test]
fn migration_json_missing_fields_use_go_zero_values() {
    assert_eq!(
        serde_json::from_str::<ReplicationConfig>("{}").unwrap(),
        ReplicationConfig::default()
    );
    assert_eq!(serde_json::from_str::<Rule>("{}").unwrap(), Rule::default());
    assert_eq!(
        serde_json::from_str::<RegionStats>("{}").unwrap(),
        RegionStats::default()
    );
}

/// API 嵌套 DTO 保留 Go 的嵌入 protobuf、omitempty、容量/时长和时间格式。
#[test]
fn migration_nested_api_types_keep_go_json_contract() {
    let mut store = metapb::Store {
        id: 7,
        address: "tikv-1:20160".into(),
        state: metapb::StoreState::Offline,
        ..Default::default()
    };
    store.mut_labels().push(metapb::StoreLabel {
        key: "zone".into(),
        value: "z1".into(),
        ..Default::default()
    });
    let status = StoreStatus {
        Capacity: 1024,
        Available: 512,
        UsedSize: 512,
        LeaderCount: 2,
        StartTS: Some("2026-08-02T01:02:03.12Z".parse().unwrap()),
        Uptime: Some(Box::new(Duration {
            Duration: 5_000_000_000,
        })),
        ..Default::default()
    };
    let info = StoreInfo {
        Store: Some(Box::new(MetaStore {
            Store: Some(Box::new(store)),
            StateName: "Offline".into(),
        })),
        Status: Some(Box::new(status)),
    };
    let json = serde_json::to_value(&info).unwrap();
    assert_eq!(json["store"]["id"], 7);
    assert_eq!(json["store"]["address"], "tikv-1:20160");
    assert_eq!(json["store"]["state"], 1);
    assert_eq!(json["store"]["labels"][0]["key"], "zone");
    assert_eq!(json["store"]["state_name"], "Offline");
    assert_eq!(json["status"]["capacity"], "1KiB");
    assert_eq!(json["status"]["available"], "512B");
    assert_eq!(json["status"]["start_ts"], "2026-08-02T01:02:03.12Z");
    assert_eq!(json["status"]["uptime"], "5s");
    assert!(json["status"].get("sending_snap_count").is_none());

    let decoded: StoreInfo = serde_json::from_value(json).unwrap();
    assert_eq!(
        decoded.Store.as_ref().unwrap().Store.as_ref().unwrap().id,
        7
    );
    assert_eq!(decoded.Status.as_ref().unwrap().Capacity, 1024);
    assert_eq!(
        decoded
            .Status
            .as_ref()
            .unwrap()
            .Uptime
            .as_ref()
            .unwrap()
            .Duration,
        5_000_000_000
    );

    let peer = metapb::Peer {
        id: 11,
        store_id: 7,
        role: metapb::PeerRole::Learner,
        ..Default::default()
    };
    let meta_peer = MetaPeer {
        Peer: Some(Box::new(peer)),
        RoleName: "Learner".into(),
        IsLearner: true,
    };
    let region = RegionInfo {
        ID: 9,
        StartKey: "61".into(),
        EndKey: "62".into(),
        RegionEpoch: Some(Box::new(metapb::RegionEpoch {
            conf_ver: 3,
            version: 4,
            ..Default::default()
        })),
        Peers: vec![meta_peer.clone()],
        Leader: meta_peer.clone(),
        DownPeers: vec![PDPeerStats {
            PeerStats: Some(Box::new(pdpb::PeerStats {
                down_seconds: 30,
                ..Default::default()
            })),
            Peer: meta_peer,
        }],
        WrittenBytes: 100,
        ReplicationStatus: Some(Box::new(ReplicationStatus {
            State: "simple-majority".into(),
            StateID: 42,
        })),
        ..Default::default()
    };
    let regions = RegionsInfo {
        Count: 1,
        Regions: vec![region],
    };
    let json = serde_json::to_value(&regions).unwrap();
    assert_eq!(json["regions"][0]["epoch"]["conf_ver"], 3);
    assert_eq!(json["regions"][0]["peers"][0]["role"], 1);
    assert_eq!(json["regions"][0]["peers"][0]["role_name"], "Learner");
    assert_eq!(json["regions"][0]["down_peers"][0]["down_seconds"], 30);
    assert_eq!(json["regions"][0]["down_peers"][0]["peer"]["id"], 11);
    assert!(json["regions"][0].get("pending_peers").is_none());

    let decoded: RegionsInfo = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.Regions[0].RegionEpoch.as_ref().unwrap().version, 4);
    assert_eq!(
        decoded.Regions[0].Peers[0].Peer.as_ref().unwrap().role,
        metapb::PeerRole::Learner
    );
    assert_eq!(
        decoded.Regions[0].DownPeers[0]
            .PeerStats
            .as_ref()
            .unwrap()
            .down_seconds,
        30
    );
}

/// 构造带指定 id 与键范围的 Region 测试对象。
fn region(id: u64, start: &[u8], end: &[u8]) -> crate::region_tree::Region {
    NewRegionInfo(
        metapb::Region {
            id,
            start_key: start.to_vec(),
            end_key: end.to_vec(),
            ..Default::default()
        },
        None,
    )
}

/// StringSlice 与 Go 一致：逗号拼接 JSON，非法输入不破坏已有内容。
#[test]
fn migration_string_slice_matches_go_json_contract() {
    let labels = StringSlice(vec!["zone".into(), "rack".into()]);
    assert_eq!(labels.MarshalJSON().unwrap(), br#""zone,rack""#);
    assert_eq!(serde_json::to_string(&labels).unwrap(), r#""zone,rack""#);

    let mut decoded = StringSlice(vec!["unchanged".into()]);
    decoded.UnmarshalJSON(br#""zone,rack""#).unwrap();
    assert_eq!(decoded.0, ["zone", "rack"]);
    decoded.UnmarshalJSON(br#""""#).unwrap();
    assert!(decoded.0.is_empty());

    decoded.0.push("preserved".into());
    assert!(decoded.UnmarshalJSON(b"not-json").is_err());
    assert_eq!(decoded.0, ["preserved"]);
}

/// RegionTree：重叠 Region 被替换，ScanRange 按键序返回并可限流。
#[test]
fn migration_region_tree_replaces_overlaps_and_scans_in_key_order() {
    let mut tree = RegionTree::default();
    tree.SetRegion(region(1, b"a", b"c"));
    tree.SetRegion(region(2, b"e", b"g"));
    // region 3 与 1、2 重叠，应替换为仅保留 3。
    tree.SetRegion(region(3, b"b", b"f"));
    assert_eq!(
        tree.Regions.iter().map(|r| r.Meta.id).collect::<Vec<_>>(),
        [3]
    );

    tree.SetRegion(region(4, b"g", b"i"));
    tree.SetRegion(region(5, b"", b"a"));
    assert_eq!(
        tree.ScanRange(b"".to_vec(), b"".to_vec(), 0)
            .iter()
            .map(|r| r.Meta.id)
            .collect::<Vec<_>>(),
        [5, 3, 4]
    );
    assert_eq!(
        tree.ScanRange(b"a".to_vec(), b"h".to_vec(), 2)
            .iter()
            .map(|r| r.Meta.id)
            .collect::<Vec<_>>(),
        [3, 4]
    );
    // The Go implementation checks limit == 0, so a negative limit returns no rows.
    // Go 实现只把 limit == 0 视为不限制；负数 limit 返回空。
    assert!(tree.ScanRange(b"".to_vec(), b"".to_vec(), -1).is_empty());
}

/// ReplicationConfig 与 Rule 的 JSON 字段名/布尔字符串/枚举值与 PD 一致。
#[test]
fn migration_configuration_and_placement_keep_pd_json_shape() {
    let config = ReplicationConfig {
        MaxReplicas: 3,
        LocationLabels: StringSlice(vec!["zone".into(), "rack".into()]),
        StrictlyMatchLabel: true,
        EnablePlacementRules: false,
        EnablePlacementRulesCache: true,
        IsolationLevel: "zone".into(),
    };
    assert_eq!(
        serde_json::to_value(config).unwrap(),
        serde_json::json!({
            "max-replicas": 3,
            "location-labels": "zone,rack",
            "strictly-match-label": "true",
            "enable-placement-rules": "false",
            "enable-placement-rules-cache": "true",
            "isolation-level": "zone"
        })
    );

    assert_eq!(Voter, PeerRoleType::Voter);
    let rule = Rule {
        GroupID: "g".into(),
        ID: "r".into(),
        Role: Voter,
        Count: 1,
        LabelConstraints: vec![LabelConstraint {
            Key: "zone".into(),
            Op: LabelConstraintOp::In,
            Values: vec!["z1".into()],
        }],
        ..Default::default()
    };
    let json = serde_json::to_value(rule).unwrap();
    assert_eq!(json["group_id"], "g");
    assert_eq!(json["role"], "voter");
    // The local Go Rule has no witness field (unlike pd/client/http.Rule).
    assert!(json.get("is_witness").is_none());
    assert_eq!(json["label_constraints"][0]["op"], "in");
    assert!(json.get("start_key").is_some());
    assert!(json.get("StartKey").is_none());
}

/// Go string aliases preserve forward-compatible PD enum values.
#[test]
fn placement_string_types_preserve_unknown_values() {
    assert_eq!(
        serde_json::to_string(&PeerRoleType::default()).unwrap(),
        r#""""#
    );
    assert_eq!(
        serde_json::to_value(Rule::default()).unwrap()["role"],
        serde_json::json!("")
    );

    let role: PeerRoleType = serde_json::from_str(r#""witness""#).unwrap();
    assert_eq!(role, PeerRoleType::Unknown("witness".into()));
    assert_eq!(serde_json::to_string(&role).unwrap(), r#""witness""#);

    let op: LabelConstraintOp = serde_json::from_str(r#""matches""#).unwrap();
    assert_eq!(op, LabelConstraintOp::Unknown("matches".into()));
    assert_eq!(serde_json::to_string(&op).unwrap(), r#""matches""#);
}

/// Local Go Rule ignores unknown witness input; PD HTTP Rule retains it.
#[test]
fn migration_local_and_http_rules_keep_distinct_witness_contracts() {
    use crate::placement::HttpRule;

    for witness in [serde_json::json!(true), serde_json::json!({"future": 1})] {
        let rule: Rule = serde_json::from_value(serde_json::json!({
            "is_witness": witness
        }))
        .unwrap();
        assert_eq!(rule, Rule::default());
        assert!(
            serde_json::to_value(rule)
                .unwrap()
                .get("is_witness")
                .is_none()
        );
    }
    assert_eq!(
        serde_json::to_value(HttpRule::default()).unwrap()["is_witness"],
        false
    );
    let rule: HttpRule = serde_json::from_str(r#"{"is_witness":true}"#).unwrap();
    assert!(rule.IsWitness);
    assert_eq!(serde_json::to_value(rule).unwrap()["is_witness"], true);
    assert!(serde_json::from_str::<HttpRule>(r#"{"is_witness":1}"#).is_err());
}

/// RegionStats 中 Store ID 映射键序列化为字符串。
#[test]
fn migration_region_statistics_use_stringified_store_ids() {
    let empty = serde_json::to_value(RegionStats::default()).unwrap();
    assert!(empty["store_leader_count"].is_null());
    assert!(empty["store_peer_keys"].is_null());

    let stats = RegionStats {
        Count: 2,
        StoreLeaderCount: HashMap::from([(42, 1)]),
        ..Default::default()
    };
    let json = serde_json::to_value(stats).unwrap();
    assert_eq!(json["count"], 2);
    assert_eq!(json["store_leader_count"]["42"], 1);
}
