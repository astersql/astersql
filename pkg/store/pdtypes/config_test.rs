// Copyright 2026 AsterSQL.

use crate::config::ReplicationConfig;

#[test]
fn replication_config_accepts_null_for_string_encoded_booleans() {
    let config: ReplicationConfig = serde_json::from_str(
        r#"{
            "strictly-match-label": null,
            "enable-placement-rules": null,
            "enable-placement-rules-cache": null
        }"#,
    )
    .expect("Go encoding/json accepts null for primitive bool fields tagged with ,string");

    assert!(!config.StrictlyMatchLabel);
    assert!(!config.EnablePlacementRules);
    assert!(!config.EnablePlacementRulesCache);

    let quoted_null: ReplicationConfig = serde_json::from_str(r#"{"strictly-match-label":"null"}"#)
        .expect("Go encoding/json accepts quoted null through the ,string option");
    assert!(!quoted_null.StrictlyMatchLabel);
}
