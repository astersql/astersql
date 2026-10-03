// Copyright 2026 AsterSQL.

use std::collections::HashMap;
use std::sync::Arc;

use super::*;

fn sample_server_info() -> ServerInfo {
    ServerInfo {
        StaticInfo: StaticInfo {
            VersionInfo: VersionInfo {
                Version: "v".into(),
                GitHash: "g".into(),
            },
            ID: "id".into(),
            IP: "127.0.0.1".into(),
            Port: 4000,
            StatusPort: 10080,
            Lease: "45s".into(),
            StartTimestamp: 7,
            Keyspace: String::new(),
            AssumedKeyspace: String::new(),
            ServerIDGetter: Some(Arc::new(|| 9)),
            JSONServerID: 0,
        },
        DynamicInfo: DynamicInfo {
            Labels: HashMap::from([("old".into(), "label".into())]),
        },
    }
}

#[test]
fn marshal_matches_go_field_omission_and_escaping() {
    let mut info = sample_server_info();
    info.DynamicInfo.Labels = HashMap::from([("<".into(), "\u{2028}&\u{0008}".into())]);

    let encoded = info.Marshal().unwrap();

    assert_eq!(
        encoded,
        br#"{"version":"v","git_hash":"g","ddl_id":"id","ip":"127.0.0.1","listening_port":4000,"status_port":10080,"lease":"45s","start_timestamp":7,"server_id":9,"labels":{"\u003c":"\u2028\u0026\b"}}"#
    );
    assert_eq!(info.StaticInfo.JSONServerID, 9);
}

#[test]
#[should_panic(expected = "ServerIDGetter")]
fn marshal_requires_server_id_getter_like_go() {
    let mut info = ServerInfo::default();
    let _ = info.Marshal();
}

#[test]
fn unmarshal_matches_go_partial_update_and_unsigned_range() {
    let mut info = sample_server_info();
    info.Unmarshal(
        br#"{"SERVER_ID":18446744073709551615,"labels":{"new":"value"},"future":[1.5,true,null]}"#,
    )
    .unwrap();

    assert_eq!(info.StaticInfo.VersionInfo.Version, "v");
    assert_eq!(info.StaticInfo.ID, "id");
    assert_eq!(info.StaticInfo.Port, 4000);
    assert_eq!(info.StaticInfo.JSONServerID, u64::MAX);
    assert_eq!(info.StaticInfo.ServerIDGetter.as_ref().unwrap()(), u64::MAX);
    assert_eq!(
        info.DynamicInfo.Labels,
        HashMap::from([
            ("old".into(), "label".into()),
            ("new".into(), "value".into()),
        ])
    );
}

#[test]
fn unmarshal_matches_go_type_checks_and_unicode() {
    let mut info = sample_server_info();
    info.Unmarshal(br#"{"ddl_id":"\ud83d\ude00","ip":"\ud800"}"#)
        .unwrap();
    assert_eq!(info.StaticInfo.ID, "😀");
    assert_eq!(info.StaticInfo.IP, "�");

    assert!(info.Unmarshal(br#"{"listening_port":-1}"#).is_err());
    assert!(info.Unmarshal(br#"{"status_port":4294967296}"#).is_err());
    assert!(info.Unmarshal(br#"{"labels":{"bad":true}}"#).is_err());
}

#[test]
fn unmarshal_processes_duplicate_fields_and_continues_after_type_errors() {
    let mut info = sample_server_info();
    info.Unmarshal(br#"{"server_id":1,"SERVER_ID":2}"#).unwrap();
    assert_eq!(info.StaticInfo.JSONServerID, 2);

    let error = info
        .Unmarshal(br#"{"ddl_id":123,"ip":"updated"}"#)
        .unwrap_err();
    assert!(error.to_string().contains("ddl_id"));
    assert_eq!(info.StaticInfo.ID, "id");
    assert_eq!(info.StaticInfo.IP, "updated");
}

#[test]
fn topology_json_accepts_go_zero_values_and_unknown_fields() {
    let topology =
        TopologyInfo::Unmarshal(br#"{"labels":{"zone":"z1"},"future":{"ratios":[1e2,false]}}"#)
            .unwrap();

    assert_eq!(topology.VersionInfo, VersionInfo::default());
    assert_eq!(topology.StatusPort, 0);
    assert_eq!(topology.Labels.get("zone").map(String::as_str), Some("z1"));
}

#[test]
fn clone_keeps_static_getter_and_deep_copies_labels() {
    let info = sample_server_info();
    let mut cloned = info.Clone();
    cloned
        .DynamicInfo
        .Labels
        .insert("new".into(), "value".into());

    assert_eq!(cloned.StaticInfo.ServerIDGetter.as_ref().unwrap()(), 9);
    assert!(!info.DynamicInfo.Labels.contains_key("new"));
}

#[test]
fn static_info_identity_decoding_ignores_dynamic_fields() {
    let mut info = StaticInfo::default();
    info.Unmarshal(br#"{"DDL_ID":"local-id","labels":42,"version":"v","status_port":10080}"#)
        .unwrap();
    assert_eq!(info.ID, "local-id");
    assert_eq!(info.VersionInfo.Version, "v");
    assert_eq!(info.StatusPort, 10080);
    info.Unmarshal(b"null").unwrap();
    assert_eq!(info.ID, "local-id");
    for body in [
        br#"{"ddl_id":1}"#.as_slice(),
        br#"{"ddl_id":"local","status_port":-1}"#,
        br#"{"ddl_id":"local","version":1}"#,
        b"[]",
        b"true",
        br#"{"ddl_id":"local"} {}"#,
    ] {
        assert!(StaticInfo::default().Unmarshal(body).is_err(), "{body:?}");
    }
    let mut info = StaticInfo::default();
    info.Unmarshal(br#"{"ddl_id":"old","ddl_id":null,"DDL_ID":"new"}"#)
        .unwrap();
    assert_eq!(info.ID, "new");
}
