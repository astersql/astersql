// Copyright 2026 AsterSQL.

use crate::Context;
use crate::utils::{
    GetPlacementRules, PeerRoleType, PlacementHttpClient, SearchPlacementRule, encode_bytes,
};
use astersql_errors::SharedError;

struct StaticPlacementHttp(Vec<u8>);

impl PlacementHttpClient for StaticPlacementHttp {
    fn get_placement_rules(
        &self,
        _ctx: &Context,
        _url: &str,
    ) -> Result<(u16, Vec<u8>), SharedError> {
        Ok((200, self.0.clone()))
    }
}

#[test]
fn placement_rule_preserves_go_pdtypes_contract() {
    let body = br#"[{
        "group_id":"g","id":"r","index":7,"override":true,
        "start_key":"aa","end_key":"bb","role":"witness","count":4294967296,
        "label_constraints":[{"key":"zone","op":"in","values":["z1"]}],
        "location_labels":["zone"],"isolation_level":"zone",
        "version":9,"create_timestamp":10
    }]"#;
    let rules = GetPlacementRules(
        &Context::new(),
        "pd",
        false,
        &StaticPlacementHttp(body.to_vec()),
    )
    .unwrap();

    let rule = &rules[0];
    assert_eq!(rule.Index, 7);
    assert!(rule.Override);
    assert_eq!(rule.Role, PeerRoleType::Unknown("witness".into()));
    assert_eq!(rule.Count, 4_294_967_296);
    assert_eq!(rule.LabelConstraints[0].Key, "zone");
    assert_eq!(rule.LocationLabels, ["zone"]);
    assert_eq!(rule.IsolationLevel, "zone");
    assert_eq!(rule.Version, 9);
    assert_eq!(rule.CreateTimestamp, 10);
}

#[test]
fn search_placement_rule_accepts_go_api_v2_raw_prefix() {
    let table_id = 42_i64;
    let mut decoded = vec![b'r', 0, 0, 1, b't'];
    decoded.extend_from_slice(&((table_id as u64) ^ (1_u64 << 63)).to_be_bytes());
    let rule = crate::utils::Rule {
        StartKeyHex: hex::encode(encode_bytes(&decoded)),
        Role: PeerRoleType::Voter,
        ..Default::default()
    };

    assert!(SearchPlacementRule(table_id, &[rule], PeerRoleType::Voter).is_some());
}
