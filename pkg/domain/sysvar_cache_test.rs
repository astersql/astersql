// Copyright 2026 AsterSQL.

use crate::sysvar_cache::{SysVarCache, SysVarDefinition, SysVarSource};
use std::collections::BTreeMap;

struct Source(BTreeMap<String, String>);

impl SysVarSource for Source {
    fn table_values(&self) -> Result<BTreeMap<String, String>, String> {
        Ok(self.0.clone())
    }
}

fn definition(name: &str, global_scope: bool) -> SysVarDefinition {
    SysVarDefinition {
        name: name.into(),
        default_value: format!("{name}-default"),
        skip_session_init: false,
        global_scope,
        initialized_from_config: false,
    }
}

#[test]
fn rebuild_matches_go_validation_and_set_global_filtering() {
    let cache = SysVarCache::default();
    let definitions = [
        definition("normal", true),
        definition("skip-cache", true),
        definition("no-callback", true),
        definition("session-only", false),
    ];
    let source = Source(BTreeMap::from([
        ("normal".into(), "raw".into()),
        ("skip-cache".into(), "skipped".into()),
        ("no-callback".into(), "plain".into()),
        ("session-only".into(), "local".into()),
    ]));
    let mut callbacks = Vec::new();

    cache
        .rebuild_with_policy(
            &source,
            &definitions,
            &BTreeMap::new(),
            |name, value| format!("validated:{name}:{value}"),
            |name| name == "normal",
            |name, value| {
                callbacks.push((name.to_owned(), value.to_owned()));
                Ok(())
            },
        )
        .unwrap();

    assert_eq!(
        callbacks,
        [("normal".into(), "validated:normal:raw".into())]
    );
    assert_eq!(cache.global_var("normal").unwrap(), "raw");
    assert_eq!(cache.global_var("skip-cache").unwrap(), "skipped");
    assert_eq!(cache.global_var("no-callback").unwrap(), "plain");
    assert!(cache.global_var("session-only").is_err());
    assert_eq!(cache.session_cache().unwrap()["session-only"], "local");
}

#[test]
fn rebuild_uses_defaults_config_overrides_and_returns_independent_session_copy() {
    let cache = SysVarCache::default();
    let mut initialized = definition("from-config-init", true);
    initialized.initialized_from_config = true;
    let mut skipped = definition("skip-session", true);
    skipped.skip_session_init = true;
    let definitions = [definition("override", true), initialized, skipped];
    let source = Source(BTreeMap::from([
        ("override".into(), "table".into()),
        ("from-config-init".into(), "table-ignored".into()),
    ]));

    cache
        .rebuild(
            &source,
            &definitions,
            &BTreeMap::from([
                ("override".into(), "configured".into()),
                ("missing".into(), "must-not-be-added".into()),
            ]),
            |_name, _value| Ok(()),
        )
        .unwrap();

    let mut copy = cache.session_cache().unwrap();
    assert_eq!(copy["override"], "configured");
    assert_eq!(copy["from-config-init"], "from-config-init-default");
    assert!(!copy.contains_key("skip-session"));
    copy.insert("override".into(), "mutated-copy".into());
    assert_eq!(cache.session_cache().unwrap()["override"], "configured");
    assert_eq!(
        cache.global_var("skip-session").unwrap(),
        "skip-session-default"
    );
}

#[test]
fn go_merge_43_rebuild_reapplies_unchanged_internal_summary_callback() {
    let cache = SysVarCache::default();
    let source = Source(BTreeMap::from([(
        "tidb_stmt_summary_internal_query".into(),
        "OFF".into(),
    )]));
    let definitions = [definition("tidb_stmt_summary_internal_query", true)];
    let mut applied = Vec::new();

    for _ in 0..2 {
        cache
            .rebuild(&source, &definitions, &BTreeMap::new(), |name, value| {
                applied.push((name.to_owned(), value.to_owned()));
                Ok(())
            })
            .unwrap();
    }

    assert_eq!(applied.len(), 2);
    assert!(
        applied
            .iter()
            .all(|(name, value)| { name == "tidb_stmt_summary_internal_query" && value == "OFF" })
    );
    assert_eq!(
        cache.global_var("tidb_stmt_summary_internal_query"),
        Ok("OFF".into())
    );
}
