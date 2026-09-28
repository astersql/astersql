// Copyright 2026 AsterSQL.

use crate::placement::{LabelConstraintOp, PeerRoleType, Rule};

#[test]
fn placement_string_aliases_accept_json_null_as_go_zero_value() {
    let rule: Rule = serde_json::from_str(
        r#"{
            "role": null,
            "label_constraints": [{"op": null}]
        }"#,
    )
    .expect("Go encoding/json accepts null for string aliases");

    assert_eq!(rule.Role, PeerRoleType::Unknown(String::new()));
    assert_eq!(rule.LabelConstraints[0].Op, LabelConstraintOp::Empty);
}
