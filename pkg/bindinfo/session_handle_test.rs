// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 聚焦 Go sessionBindingHandle 的可执行单元契约。依赖完整 SQL 会话、optimizer
// 与 testkit 的场景位于独立 `astersql-bindinfo-tests` crate，避免依赖环。

struct Validator;

impl crate::BindingValidator for Validator {
    fn validate_binding_sql(&self, sql: &str) -> crate::Result<()> {
        if sql.trim().is_empty() {
            Err(crate::BindError("empty binding SQL".to_owned()))
        } else {
            Ok(())
        }
    }
}

fn binding(original_sql: &str, db: &str) -> crate::Binding {
    crate::Binding {
        OriginalSQL: original_sql.to_owned(),
        Db: db.to_owned(),
        BindSQL: "select /*+ use_index(t, idx) */ * from t".to_owned(),
        Status: crate::StatusEnabled.to_owned(),
        ..crate::Binding::default()
    }
}

fn digest(sql: &str) -> String {
    astersql_parser::DigestNormalized(sql).String().to_owned()
}

#[test]
fn canonical_session_binding_round_trips_state_and_drops_by_derived_digest() {
    let handle = crate::NewSessionBindingHandle();
    handle
        .CreateSessionBinding(&Validator, vec![binding("select * from t", "TeSt")])
        .unwrap();

    let stored = handle.GetAllSessionBindings();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].Db, "test");
    assert!(!stored[0].CreateTime.is_zero());
    assert_eq!(stored[0].CreateTime, stored[0].UpdateTime);
    assert_eq!(
        stored[0].Hint.Hints,
        ["use_index(@`sel_1` `test`.`t` `idx`)"]
    );

    let no_db_digest = crate::noDBDigestFromBinding(&stored[0]).unwrap();
    let (matched, found) = handle.MatchSessionBinding("test", &no_db_digest, &stored[0].TableNames);
    assert!(found);
    assert_eq!(matched.unwrap().OriginalSQL, "select * from t");

    let mut states = crate::SessionStates::default();
    handle.EncodeSessionStates(&mut states).unwrap();
    assert!(!states.Bindings.is_empty());
    assert_eq!(
        serde_json::to_value(&states).unwrap(),
        serde_json::json!({"bindings": states.Bindings})
    );

    let restored = crate::NewSessionBindingHandle();
    restored.DecodeSessionStates(&Validator, &states).unwrap();
    let restored_bindings = restored.GetAllSessionBindings();
    assert_eq!(restored_bindings.len(), 1);
    assert_eq!(restored_bindings[0].CreateTime, stored[0].CreateTime);

    restored
        .DropSessionBinding(&[digest("select * from t")])
        .unwrap();
    assert!(restored.GetAllSessionBindings().is_empty());
    handle.Close();
    assert!(handle.GetAllSessionBindings().is_empty());
}

#[test]
fn create_is_atomic_and_replaces_same_normalized_sql_like_go() {
    let handle = crate::NewSessionBindingHandle();
    let mut invalid = binding("select * from invalid", "test");
    invalid.BindSQL.clear();
    assert!(
        handle
            .CreateSessionBinding(
                &Validator,
                vec![binding("select * from t", "test"), invalid]
            )
            .is_err()
    );
    assert!(handle.GetAllSessionBindings().is_empty());

    handle
        .CreateSessionBinding(&Validator, vec![binding("select * from t", "test")])
        .unwrap();
    let mut replacement = binding("select * from t", "TEST");
    replacement.BindSQL = "select /*+ hash_agg() */ * from t".to_owned();
    handle
        .CreateSessionBinding(&Validator, vec![replacement])
        .unwrap();
    let bindings = handle.GetAllSessionBindings();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].BindSQL, "select /*+ hash_agg() */ * from t");
}

#[test]
fn session_state_empty_invalid_and_old_style_paths_match_go() {
    let empty = crate::NewSessionBindingHandle();
    let mut empty_states = crate::SessionStates::default();
    empty.EncodeSessionStates(&mut empty_states).unwrap();
    assert!(empty_states.Bindings.is_empty());
    assert_eq!(
        serde_json::to_value(&empty_states).unwrap(),
        serde_json::json!({})
    );
    empty
        .DecodeSessionStates(&Validator, &empty_states)
        .unwrap();

    let invalid = crate::SessionStates {
        Bindings: "not-json".to_owned(),
    };
    assert!(empty.DecodeSessionStates(&Validator, &invalid).is_err());

    let old_binding = crate::Binding {
        BindSQL: "select /*+ use_index(t, idx) */ * from t".to_owned(),
        Status: crate::StatusEnabled.to_owned(),
        ..crate::Binding::default()
    };
    let states = crate::SessionStates {
        Bindings: serde_json::json!([{
            "OriginalSQL": "select * from t",
            "Db": "TeSt",
            "Bindings": [old_binding]
        }])
        .to_string(),
    };
    let restored = crate::NewSessionBindingHandle();
    restored.DecodeSessionStates(&Validator, &states).unwrap();
    let bindings = restored.GetAllSessionBindings();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].OriginalSQL, "select * from t");
    assert_eq!(bindings[0].Db, "TeSt");
    assert_eq!(
        bindings[0].Hint.Hints,
        ["use_index(@`sel_1` `test`.`t` `idx`)"]
    );
}

#[test]
fn session_bind_info_key_string_matches_go() {
    assert_eq!(
        crate::sessionBindInfoKeyType_String(crate::SessionBindInfoKeyType),
        "session_bindinfo"
    );
    assert_eq!(
        crate::String(crate::SessionBindInfoKeyType),
        "session_bindinfo"
    );
}
