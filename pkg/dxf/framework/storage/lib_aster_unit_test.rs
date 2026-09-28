// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn extra_params_json_omits_go_zero_values() {
    let empty = json::Marshal(&proto::ExtraParams::default()).unwrap();
    assert_eq!(String::from_utf8(empty).unwrap(), "{}");

    let params = proto::ExtraParams {
        ManualRecovery: true,
        ..proto::ExtraParams::default()
    };
    let encoded = String::from_utf8(json::Marshal(&params).unwrap()).unwrap();
    assert_eq!(encoded, r#"{"manual_recovery":true}"#);
}

#[test]
fn internal_dist_task_source_name_matches_go() {
    assert_eq!(kv::InternalDistTask, "DistTask");
}
