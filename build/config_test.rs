// Copyright 2026 AsterSQL.

use crate::build::{NogoConfig, parse_config};

const RUST_SOURCE: &str = include_str!("config.rs");

#[test]
fn embedded_nogo_config_matches_the_go_config_shape() {
    assert_eq!(NogoConfig.len(), 137);

    let all_revive = NogoConfig
        .get("all_revive")
        .expect("all_revive must be loaded from nogo_config.json");
    assert_eq!(
        all_revive
            .ExcludeFiles
            .as_ref()
            .unwrap()
            .get("pkg/parser/parser.go"),
        Some(&"parser/parser.go code".to_owned())
    );
    assert!(all_revive.OnlyFiles.is_none());

    let exptostd = NogoConfig
        .get("exptostd")
        .expect("exptostd must be loaded from nogo_config.json");
    assert_eq!(
        exptostd.OnlyFiles.as_ref().unwrap().get("pkg/util/"),
        Some(&"util code".to_owned())
    );
}

#[test]
fn malformed_nogo_config_is_rejected() {
    let malformed = br#"{"broken":{"exclude_files":[]}}"#;
    assert!(parse_config(malformed).is_err());
}

#[test]
fn config_preserves_absent_null_and_empty_map_states() {
    assert!(RUST_SOURCE.contains("pub ExcludeFiles: Option<HashMap<String, String>>"));
    assert!(RUST_SOURCE.contains("pub OnlyFiles: Option<HashMap<String, String>>"));

    let parsed = parse_config(
        br#"{
            "absent": {},
            "null": {"only_files": null, "exclude_files": null},
            "empty": {"only_files": {}, "exclude_files": {}}
        }"#,
    )
    .expect("all three Go JSON map states are valid");
    assert!(parsed["absent"].OnlyFiles.is_none());
    assert!(parsed["absent"].ExcludeFiles.is_none());
    assert!(parsed["null"].OnlyFiles.is_none());
    assert!(parsed["null"].ExcludeFiles.is_none());
    assert!(parsed["empty"].OnlyFiles.as_ref().unwrap().is_empty());
    assert!(parsed["empty"].ExcludeFiles.as_ref().unwrap().is_empty());
}
