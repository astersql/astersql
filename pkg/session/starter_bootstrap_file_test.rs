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

#[cfg(feature = "nextgen")]
#[test]
fn starter_bootstrap_file_is_loaded_during_factory_startup() {
    use crate::runtime::CanonicalSessionFactory;
    let original = astersql_config::get_global_config();
    let original_mode = astersql_config_deploymode::Get();
    struct Restore(astersql_config::Config, astersql_config_deploymode::Mode);
    impl Drop for Restore {
        fn drop(&mut self) {
            astersql_config::store_global_config(self.0.clone());
            astersql_config_deploymode::Set(self.1).unwrap();
        }
    }
    let _restore = Restore((*original).clone(), original_mode);
    let mut config = (*original).clone();
    config.starter_params.bootstrap_file =
        format!("/missing-starter-bootstrap-{}.json", std::process::id());
    astersql_config::store_global_config(config);
    astersql_config_deploymode::Set(astersql_config_deploymode::Starter).unwrap();
    let storage = || {
        std::sync::Arc::try_unwrap(
            astersql_store_mockstore_mockstorage::NewMockStorage(
                astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
                Some(astersql_store_mockstore_mockstorage::KeyspaceMeta {
                    Name: "SYSTEM".into(),
                    Id: 0,
                }),
            )
            .unwrap(),
        )
        .ok()
        .unwrap()
    };
    let result = CanonicalSessionFactory::from_storage_for_test(storage());
    match result {
        Err(error) => assert!(
            error.to_string().contains("read starter bootstrap file"),
            "{error}"
        ),
        Ok(factory) => {
            factory.domain().close();
            panic!("starter startup accepted a missing configured bootstrap file");
        }
    }
    let path = std::env::temp_dir().join(format!("starter-startup-{}.json", std::process::id()));
    struct Remove(std::path::PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _remove = Remove(path.clone());
    std::fs::write(&path, r#"{"version":2,"bootstrap":["INSERT INTO mysql.tidb VALUES ('starter_factory','boot','test')"]}"#).unwrap();
    let mut config = (*original).clone();
    config.starter_params.bootstrap_file = path.to_str().unwrap().into();
    astersql_config::store_global_config(config);
    let factory = CanonicalSessionFactory::from_storage_for_test(storage()).unwrap();
    let session = factory.create_session();
    assert_eq!(value(&session, "starter_factory"), Some("boot".into()));
    assert_eq!(get_starter_bootstrap_version(&session).unwrap(), 2);
    assert_eq!(
        get_store_starter_bootstrap_version(factory.domain()).unwrap(),
        2
    );
    let mut record = session
        .execute(&format!(
            "SELECT COMMENT FROM mysql.tidb WHERE VARIABLE_NAME='{VERSION_VAR}'"
        ))
        .unwrap()
        .remove(0);
    assert_eq!(
        record.next_row().unwrap(),
        Some(vec![
            "Starter bootstrap file version. Do not delete.".into()
        ])
    );
    record.close().unwrap();
    factory.domain().close();
}

use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
use crate::starter_bootstrap_file::*;

fn file(json: &str) -> StarterBootstrapFile {
    parse_starter_bootstrap_file(json.as_bytes()).unwrap()
}
fn value(session: &ConcreteSession, name: &str) -> Option<String> {
    let mut records = session
        .execute(&format!(
            "SELECT VARIABLE_VALUE FROM mysql.tidb WHERE VARIABLE_NAME='{}'",
            name.replace('\'', "''")
        ))
        .unwrap();
    let result = records[0].next_row().unwrap().map(|row| row[0].clone());
    records[0].close().unwrap();
    result
}
fn insertion(name: &str, value: &str) -> String {
    format!("INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('{name}', '{value}', 'test')")
}
#[test]
fn starter_manifest_validation_rendering_and_pending_upgrades() {
    let manifest = file(
        r#"{"version":3,"bootstrap":["SELECT '<keyspace>.root'"],"upgrades":[{"version":3,"sql":["SELECT '<keyspace>.v3'"]},{"version":2,"sql":[]}]}"#,
    );
    assert_eq!(
        manifest
            .upgrades
            .iter()
            .map(|u| u.version)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert_eq!(manifest.pending_upgrades(1).len(), 2);
    assert_eq!(manifest.pending_upgrades(2).len(), 1);
    assert!(manifest.pending_upgrades(3).is_empty());
    assert_eq!(
        render_starter_bootstrap_sql("SELECT '<keyspace>.root'", "ks'name"),
        "SELECT 'ks\\'name.root'"
    );
    assert_eq!(
        render_starter_bootstrap_sql("SELECT '<keyspace>'", "ks\\\n\0\r\x1a\""),
        "SELECT 'ks\\\\\\n\\0\\r\\Z\\\"'"
    );
    assert_eq!(
        file(r#"{"version":1,"bootstrap":null,"upgrades":null}"#)
            .bootstrap
            .len(),
        0
    );
    assert_eq!(
        file(r#"{"version":1,"VERSION":2,"version":null}"#).version,
        2
    );
}
#[test]
fn starter_manifest_validation_rejects_invalid_input() {
    for (input, error) in [
        (r#"{"version":1,"extra":[]}"#, "unknown field \"extra\""),
        (
            r#"{"version":0}"#,
            "bootstrap file version must be greater than 0",
        ),
        (
            r#"{"version":2,"upgrades":[{"version":2},{"version":2}]}"#,
            "duplicated upgrade version 2",
        ),
        (
            r#"{"version":2,"upgrades":[{"version":3}]}"#,
            "upgrades[0].version 3 is greater than bootstrap file version 2",
        ),
        (
            r#"{"version":2,"upgrades":[{"version":0}]}"#,
            "upgrades[0].version must be greater than 0",
        ),
        (
            r#"{"version":1,"bootstrap":["SELECT '<tenant>'"]}"#,
            "bootstrap[0] uses unsupported placeholder \"<tenant>\"",
        ),
        (
            r#"{"version":1,"bootstrap":[" "]}"#,
            "bootstrap[0] must not be empty",
        ),
        (
            r#"{"version":1,"upgrades":[{"version":1,"sql":[" "]}]}"#,
            "upgrades[0].sql[0] must not be empty",
        ),
        (
            r#"{"version":1,"upgrades":[{"version":1,"extra":1}]}"#,
            "unknown field \"extra\"",
        ),
        (
            r#"{"version":1} {}"#,
            "bootstrap file must contain a single JSON object",
        ),
        ("null", "bootstrap file version must be greater than 0"),
    ] {
        let actual = parse_starter_bootstrap_file(input.as_bytes())
            .unwrap_err()
            .to_string();
        assert!(actual.contains(error), "{input}: {actual}");
    }
    assert!(parse_starter_bootstrap_file(b"{").is_err());
}
#[test]
fn starter_bootstrap_executes_one_statement_and_restores_restricted_mode() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    session
        .execute("SET SESSION sql_mode='NO_BACKSLASH_ESCAPES'")
        .unwrap();
    execute_starter_bootstrap_sql_blocks(
        &session,
        &[insertion("starter_blocks", "<keyspace>.boot")],
        "test_keyspace",
    )
    .unwrap();
    assert_eq!(
        value(&session, "starter_blocks"),
        Some("test_keyspace.boot".into())
    );
    let error = execute_starter_bootstrap_sql_blocks(&session, &["SELECT 1; SELECT 2".into()], "")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("SQL block 0 must contain exactly one statement")
    );
    assert!(session.execute("SELECT 'ks\\'name'").is_err());
    execute_starter_bootstrap_sql_blocks(&session, &["SELECT '<keyspace>'".into()], "ks'name")
        .unwrap();
    assert!(session.execute("SELECT 'ks\\'name'").is_err());
    assert!(
        execute_starter_bootstrap_sql_blocks(&session, &["this is not SQL".into()], "")
            .unwrap_err()
            .to_string()
            .contains("parse SQL block 0")
    );
    update_starter_bootstrap_version(&session, 2).unwrap();
    assert_eq!(get_starter_bootstrap_version(&session).unwrap(), 2);
    domain.close();
}
#[test]
fn starter_initial_bootstrap_commits_only_bootstrap_and_rolls_back_failure() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let manifest = file(
        r#"{"version":3,"bootstrap":["INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('starter_initial','<keyspace>.boot','test')"],"upgrades":[{"version":3,"sql":["INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('starter_initial_upgrade','upgrade','test')"]}]}"#,
    );
    run_starter_bootstrap_locked(&session, &manifest, "test_keyspace").unwrap();
    assert_eq!(get_starter_bootstrap_version(&session).unwrap(), 3);
    assert_eq!(
        value(&session, "starter_initial"),
        Some("test_keyspace.boot".into())
    );
    assert_eq!(value(&session, "starter_initial_upgrade"), None);
    let failing = file(
        r#"{"version":4,"bootstrap":["INSERT INTO mysql.tidb VALUES ('starter_rollback','first','test')","INSERT INTO mysql.tidb VALUES ('starter_rollback','second','test')"]}"#,
    );
    assert!(run_starter_bootstrap_locked(&session, &failing, "").is_err());
    let check = ConcreteSession::new(domain.clone());
    assert_eq!(value(&check, "starter_rollback"), None);
    assert_eq!(get_starter_bootstrap_version(&check).unwrap(), 3);
    domain.close();
}
#[test]
fn starter_upgrades_run_in_order_and_skip_completed_or_older_versions() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    update_starter_bootstrap_version(&session, 1).unwrap();
    let manifest = file(
        r#"{"version":3,"upgrades":[{"version":3,"sql":["UPDATE mysql.tidb SET VARIABLE_VALUE='v3' WHERE VARIABLE_NAME='starter_order'"]},{"version":2,"sql":["INSERT INTO mysql.tidb VALUES ('starter_order','<keyspace>.v2','test')"]},{"version":1,"sql":["SELECT * FROM missing_table"]}]}"#,
    );
    upgrade_starter_bootstrap_from_version(&session, &manifest, 1, "test_keyspace").unwrap();
    assert_eq!(value(&session, "starter_order"), Some("v3".into()));
    assert_eq!(get_starter_bootstrap_version(&session).unwrap(), 3);
    upgrade_starter_bootstrap_from_version(&session, &manifest, 3, "").unwrap();
    update_starter_bootstrap_version(&session, 5).unwrap();
    upgrade_starter_bootstrap_from_version(&session, &manifest, 5, "").unwrap();
    assert_eq!(get_starter_bootstrap_version(&session).unwrap(), 5);
    upgrade_starter_bootstrap_from_version(&session, &file(r#"{"version":6}"#), 5, "").unwrap();
    assert_eq!(get_starter_bootstrap_version(&session).unwrap(), 6);
    domain.close();
}
#[test]
fn starter_upgrade_partial_failure_keeps_old_version_and_committed_statement() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    update_starter_bootstrap_version(&session, 1).unwrap();
    let manifest = file(
        r#"{"version":2,"upgrades":[{"version":2,"sql":["INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('starter_partial','first','test')","INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('starter_partial','second','test')"]}]}"#,
    );
    let error = upgrade_starter_bootstrap_from_version(&session, &manifest, 1, "").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("upgrade starter bootstrap file to version 2")
    );
    let check = ConcreteSession::new(domain.clone());
    assert_eq!(get_starter_bootstrap_version(&check).unwrap(), 1);
    assert_eq!(value(&check, "starter_partial"), Some("first".into()));
    domain.close();
}
#[test]
fn starter_store_version_gate_rechecks_lock_and_repairs_completion_after_crash() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    assert_eq!(get_store_starter_bootstrap_version(&domain).unwrap(), 0);
    let manifest = file(
        r#"{"version":3,"bootstrap":["INSERT INTO mysql.tidb VALUES ('starter_store','initialized','test')"]}"#,
    );
    reconcile_starter_bootstrap(&domain, &manifest, "", || Ok(())).unwrap();
    assert_eq!(get_store_starter_bootstrap_version(&domain).unwrap(), 3);
    assert_eq!(get_starter_bootstrap_version(&session).unwrap(), 3);
    assert_eq!(value(&session, "starter_store"), Some("initialized".into()));
    reconcile_starter_bootstrap(&domain, &manifest, "", || -> crate::SessionResult<()> {
        panic!("completion fast path acquired lock")
    })
    .unwrap();
    finish_starter_bootstrap(&domain, 0).unwrap();
    reconcile_starter_bootstrap(&domain, &manifest, "", || Ok(())).unwrap();
    assert_eq!(get_store_starter_bootstrap_version(&domain).unwrap(), 3);
    finish_starter_bootstrap(&domain, 0).unwrap();
    reconcile_starter_bootstrap(&domain, &manifest, "", || {
        finish_starter_bootstrap(&domain, 4)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(get_store_starter_bootstrap_version(&domain).unwrap(), 4);
    assert_eq!(get_starter_bootstrap_version(&session).unwrap(), 3);
    finish_starter_bootstrap(&domain, 0).unwrap();
    assert!(
        reconcile_starter_bootstrap(&domain, &manifest, "", || Err::<(), _>(
            crate::SessionError::new("injected lock failure")
        ))
        .unwrap_err()
        .to_string()
        .contains("acquire starter bootstrap file upgrade lock")
    );
    assert_eq!(get_store_starter_bootstrap_version(&domain).unwrap(), 0);
    session.execute("UPDATE mysql.tidb SET VARIABLE_VALUE='bad' WHERE VARIABLE_NAME='starter_bootstrap_version'").unwrap();
    assert!(
        get_starter_bootstrap_version(&session)
            .unwrap_err()
            .to_string()
            .contains("invalid starter bootstrap version")
    );
    domain.close();
}

#[test]
fn starter_bootstrap_config_rejects_nonempty_path_outside_starter() {
    let mut config = astersql_config::new_config();
    config.starter_params.bootstrap_file = "/etc/tidb/starter-bootstrap.json".into();
    assert!(
        config.valid().unwrap_err().to_string().contains(
            "starter-params.bootstrap-file can only be configured for starter deploy mode"
        )
    );
    config.starter_params.bootstrap_file.clear();
    config.valid().unwrap();
}

#[cfg(feature = "nextgen")]
#[test]
fn starter_manifest_loading_respects_deployment_and_reports_path_errors() {
    let original = astersql_config::get_global_config();
    let original_mode = astersql_config_deploymode::Get();
    struct Restore(
        astersql_config::Config,
        astersql_config_deploymode::Mode,
        std::path::PathBuf,
    );
    impl Drop for Restore {
        fn drop(&mut self) {
            astersql_config::store_global_config(self.0.clone());
            astersql_config_deploymode::Set(self.1).unwrap();
            if self.2.exists() {
                std::fs::remove_file(&self.2).unwrap();
            }
        }
    }
    let path = std::env::temp_dir().join(format!(
        "starter-manifest-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _restore = Restore((*original).clone(), original_mode, path.clone());
    let mut config = (*original).clone();
    config.starter_params.bootstrap_file = path.to_str().unwrap().to_owned();
    astersql_config::store_global_config(config.clone());
    astersql_config_deploymode::Set(astersql_config_deploymode::Premium).unwrap();
    assert!(load_starter_bootstrap_file().unwrap().is_none());
    astersql_config_deploymode::Set(astersql_config_deploymode::Starter).unwrap();
    assert!(
        load_starter_bootstrap_file()
            .unwrap_err()
            .to_string()
            .contains(&format!("read starter bootstrap file {}", path.display()))
    );
    std::fs::write(&path, r#"{"version":1,"bootstrap":["SELECT 1"]}"#).unwrap();
    let manifest = load_starter_bootstrap_file().unwrap().unwrap();
    assert_eq!(manifest.version, 1);
    assert_eq!(manifest.bootstrap, vec!["SELECT 1"]);
    std::fs::write(&path, r#"{"version":0}"#).unwrap();
    assert!(
        load_starter_bootstrap_file()
            .unwrap_err()
            .to_string()
            .contains(&format!("parse starter bootstrap file {}", path.display()))
    );
    config.starter_params.bootstrap_file.clear();
    astersql_config::store_global_config(config);
    assert!(load_starter_bootstrap_file().unwrap().is_none());
}
