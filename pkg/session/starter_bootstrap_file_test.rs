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
    std::fs::write(&path, r#"{"version":2,"bootstrap":["INSERT INTO mysql.tidb VALUES ('starter_factory','boot','test')", "INSERT INTO mysql.user (Host, User, authentication_string, plugin) VALUES ('%', '<keyspace>.root', '', 'mysql_native_password')"]}"#).unwrap();
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
        r#"{"version":3,"bootstrap":["INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('starter_initial','<keyspace>.boot','test')", "INSERT INTO mysql.user (Host, User, authentication_string, plugin) VALUES ('%', '<keyspace>.root', '', 'mysql_native_password')"],"upgrades":[{"version":3,"sql":["INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('starter_initial_upgrade','upgrade','test')"]}]}"#,
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
        r#"{"version":3,"bootstrap":["INSERT INTO mysql.tidb VALUES ('starter_store','initialized','test')", "INSERT INTO mysql.user (Host, User, authentication_string, plugin) VALUES ('%', '<keyspace>.root', '', 'mysql_native_password')"]}"#,
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

#[test]
fn starter_bootstrap_requires_dml_and_root_before_committing_version() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let non_dml =
        file(r#"{"version":3,"bootstrap":["CREATE TABLE test.invalid_bootstrap (id INT)"]}"#);
    assert!(
        run_starter_bootstrap_locked(&session, &non_dml, "restored_keyspace")
            .unwrap_err()
            .to_string()
            .contains("must be INSERT, REPLACE, UPDATE, or DELETE")
    );
    domain.close();
}

#[test]
fn starter_bootstrap_requires_dml_and_root_before_committing_version_missing_root() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let missing_root = file(
        r#"{"version":3,"bootstrap":["INSERT INTO mysql.tidb VALUES ('reset_probe','unexpected','test')"]}"#,
    );
    assert!(
        run_starter_bootstrap_locked(&session, &missing_root, "restored_keyspace")
            .unwrap_err()
            .to_string()
            .contains("must create 'restored_keyspace.root'@'%'")
    );
    assert_eq!(value(&session, "reset_probe"), None);
    assert_eq!(get_starter_bootstrap_version(&session).unwrap(), 0);
    domain.close();
}

fn reset_file() -> StarterBootstrapFile {
    file(
        r#"{"version":3,"bootstrap":["INSERT INTO mysql.user (Host, User, authentication_string, plugin) VALUES ('%', '<keyspace>.root', '', 'mysql_native_password')"]}"#,
    )
}
fn scalar(session: &ConcreteSession, sql: &str) -> String {
    let mut records = session.execute(sql).unwrap();
    let row = records[0].next_row().unwrap().unwrap();
    records[0].close().unwrap();
    row[0].clone()
}
fn seed_privileges(session: &ConcreteSession, user: &str) {
    for sql in [
        format!(
            "INSERT INTO mysql.columns_priv (Host, DB, User, Table_name, Column_name, Column_priv) VALUES ('%', 'test', '{user}', 't', 'c', 'Select')"
        ),
        format!("INSERT INTO mysql.db (Host, DB, User) VALUES ('%', 'test', '{user}')"),
        format!(
            "INSERT INTO mysql.default_roles (Host, User, DEFAULT_ROLE_HOST, DEFAULT_ROLE_USER) VALUES ('%', '{user}', '%', 'source_role')"
        ),
        format!(
            "INSERT INTO mysql.global_grants (User, Host, Priv) VALUES ('{user}', '%', 'BACKUP_ADMIN')"
        ),
        format!("INSERT INTO mysql.global_priv (Host, User, Priv) VALUES ('%', '{user}', '{{}}')"),
        format!(
            "INSERT INTO mysql.password_history (Host, User, Password) VALUES ('%', '{user}', 'hash')"
        ),
        format!(
            "INSERT INTO mysql.role_edges (FROM_HOST, FROM_USER, TO_HOST, TO_USER) VALUES ('%', 'source_role', '%', '{user}')"
        ),
        format!("INSERT INTO mysql.user (Host, User) VALUES ('%', '{user}')"),
        format!(
            "INSERT INTO mysql.tables_priv (Host, DB, User, Table_name, Table_priv) VALUES ('%', 'test', '{user}', 't', 'Select')"
        ),
    ] {
        session.execute(&sql).unwrap();
    }
}
fn assert_privileges(session: &ConcreteSession, user: &str, count: u64) {
    for table in PRIVILEGE_RESET_TABLES {
        let column = if table == "role_edges" {
            "TO_USER"
        } else {
            "User"
        };
        assert_eq!(
            scalar(
                session,
                &format!("SELECT COUNT(*) FROM mysql.{table} WHERE {column}='{user}'")
            ),
            count.to_string(),
            "{table}"
        );
    }
}
#[test]
fn starter_privilege_reset_validates_before_deleting_and_retries_rolled_back_bootstrap() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    seed_privileges(&session, "source_keyspace.user");
    update_starter_bootstrap_version(&session, 3).unwrap();
    for bad in [
        file(r#"{"version":3}"#),
        file(r#"{"version":3,"bootstrap":["CREATE TABLE test.reset_bad (id INT)"]}"#),
        file(r#"{"version":3,"bootstrap":["DELETE FROM mysql.user", "SELECT 1; SELECT 2"]}"#),
    ] {
        assert!(reset_privileges_locked(&session, &bad, "restored_keyspace").is_err());
        assert_privileges(&session, "source_keyspace.user", 1);
    }
    let missing = file(
        r#"{"version":3,"bootstrap":["INSERT INTO mysql.user (Host, User) VALUES ('%', '<keyspace>.not_root')"]}"#,
    );
    assert!(
        reset_privileges_locked(&session, &missing, "restored_keyspace")
            .unwrap_err()
            .to_string()
            .contains("must create 'restored_keyspace.root'@'%'")
    );
    assert_privileges(&session, "source_keyspace.user", 0);
    assert_eq!(
        scalar(
            &session,
            "SELECT COUNT(*) FROM mysql.user WHERE User='restored_keyspace.not_root'"
        ),
        "0"
    );
    let duplicate = file(
        r#"{"version":3,"bootstrap":["INSERT INTO mysql.user (Host, User) VALUES ('%', '<keyspace>.failed')", "INSERT INTO mysql.user (Host, User) VALUES ('%', '<keyspace>.failed')"]}"#,
    );
    assert!(reset_privileges_locked(&session, &duplicate, "restored_keyspace").is_err());
    assert_eq!(
        scalar(
            &session,
            "SELECT COUNT(*) FROM mysql.user WHERE User='restored_keyspace.failed'"
        ),
        "0"
    );
    reset_privileges_locked(&session, &reset_file(), "restored_keyspace").unwrap();
    assert_eq!(
        scalar(
            &session,
            "SELECT COUNT(*) FROM mysql.user WHERE Host='%' AND User='restored_keyspace.root' AND authentication_string=''"
        ),
        "1"
    );
    assert_eq!(get_starter_bootstrap_version(&session).unwrap(), 3);
    assert_eq!(
        scalar(
            &session,
            "SELECT COUNT(*) FROM mysql.password_history WHERE User='source_keyspace.user'"
        ),
        "1"
    );
    domain.close();
}
#[test]
fn starter_privilege_reset_batches_large_user_table_and_preserves_password_history() {
    use std::sync::atomic::Ordering;
    let (domain, session) = CreateAnalyzeSession().unwrap();
    seed_privileges(&session, "source_keyspace.user");
    let values = (0..1024)
        .map(|i| format!("('%','source_keyspace.user{i:04}')"))
        .collect::<Vec<_>>()
        .join(",");
    session
        .execute(&format!(
            "INSERT INTO mysql.user (Host, User) VALUES {values}"
        ))
        .unwrap();
    session.execute("CREATE TABLE test.t (c INT)").unwrap();
    struct Restore(u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            astersql_kv::TxnTotalSizeLimit.store(self.0, Ordering::SeqCst);
        }
    }
    let _restore = Restore(astersql_kv::TxnTotalSizeLimit.swap(32 * 1024, Ordering::SeqCst));
    assert!(
        session
            .execute("DELETE FROM mysql.user")
            .err()
            .expect("unbounded delete must exceed the transaction size limit")
            .to_string()
            .to_ascii_lowercase()
            .contains("too large")
    );
    session.execute("ROLLBACK").unwrap();
    reset_privileges_locked(&session, &reset_file(), "restored_keyspace").unwrap();
    assert_privileges(&session, "source_keyspace.user", 0);
    assert_eq!(
        scalar(
            &session,
            "SELECT COUNT(*) FROM mysql.user WHERE User LIKE 'source_keyspace.user%'"
        ),
        "0"
    );
    assert_eq!(
        scalar(
            &session,
            "SELECT COUNT(*) FROM mysql.password_history WHERE User='source_keyspace.user'"
        ),
        "1"
    );
    session
        .execute("CREATE USER 'source_keyspace.user'@'%'")
        .unwrap();
    let mut user_session = ConcreteSession::new(domain.clone());
    user_session
        .AuthenticateUserForTest(&astersql_parser_auth::parser::auth::auth::UserIdentity {
            username: "source_keyspace.user".into(),
            hostname: "localhost".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(
        user_session
            .execute("SELECT c FROM test.t")
            .err()
            .expect("copied grants must be removed")
            .to_string()
            .contains("SELECT command denied")
    );
    domain.close();
}
#[test]
fn starter_privilege_reset_metadata_parses_go_boolean_forms_and_preserves_cas_values() {
    for (values, pending) in [
        (vec![], 0),
        (vec![(RESTORE_RESET_DONE_KEY, "False")], 1),
        (vec![(RESTORE_RESET_DONE_KEY, "true")], 0),
        (vec![(BRANCH_RESET_DONE_KEY, "False")], 1),
        (vec![(BRANCH_RESET_DONE_KEY, "true")], 0),
        (
            vec![
                (BRANCH_RESET_DONE_KEY, "true"),
                (RESTORE_RESET_DONE_KEY, "False"),
            ],
            1,
        ),
        (
            vec![
                (BRANCH_RESET_DONE_KEY, "False"),
                (RESTORE_RESET_DONE_KEY, "false"),
            ],
            2,
        ),
    ] {
        let state = parse_privilege_reset(Some(StarterKeyspaceMeta {
            name: "ks".into(),
            config: values
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }))
        .unwrap();
        assert_eq!(state.pending_markers.len(), pending);
        let params = privilege_reset_completion_params(&state);
        for (key, value) in state.pending_markers {
            assert_eq!(params.Config[&key], Some("True".into()));
            assert_eq!(params.Preconditions[&key], Some(value));
        }
    }
    for value in ["0", "f", "F", "FALSE", "false", "False"] {
        assert_eq!(
            parse_privilege_reset(Some(StarterKeyspaceMeta {
                name: "ks".into(),
                config: [(RESTORE_RESET_DONE_KEY.into(), value.into())].into()
            }))
            .unwrap()
            .pending_markers
            .len(),
            1
        );
    }
    for value in ["", "1", "t", "T", "TRUE", "true", "True"] {
        assert!(
            parse_privilege_reset(Some(StarterKeyspaceMeta {
                name: "ks".into(),
                config: [(RESTORE_RESET_DONE_KEY.into(), value.into())].into()
            }))
            .unwrap()
            .pending_markers
            .is_empty()
        );
    }
    assert!(
        parse_privilege_reset(Some(StarterKeyspaceMeta {
            name: "ks".into(),
            config: [(RESTORE_RESET_DONE_KEY.into(), "invalid".into())].into()
        }))
        .unwrap_err()
        .to_string()
        .contains("invalid starter privilege reset marker")
    );
}

struct ResetPd {
    snapshot: std::cell::RefCell<StarterKeyspaceMeta>,
    current: std::cell::RefCell<Option<StarterKeyspaceMeta>>,
    loads: std::cell::Cell<usize>,
    updates: std::cell::Cell<usize>,
    fail_update: std::cell::Cell<bool>,
    fail_load: std::cell::Cell<bool>,
}
impl StarterPrivilegeResetMetadata for ResetPd {
    fn snapshot(&self) -> crate::SessionResult<Option<StarterKeyspaceMeta>> {
        Ok(Some(self.snapshot.borrow().clone()))
    }
    fn refresh(&self, name: &str) -> crate::SessionResult<Option<StarterKeyspaceMeta>> {
        self.loads.set(self.loads.get() + 1);
        assert_eq!(name, "restored_keyspace");
        if self.fail_load.get() {
            return Err(crate::SessionError::new(
                "PD client is required to refresh starter privilege reset metadata",
            ));
        }
        Ok(self.current.borrow().clone())
    }
    fn complete(&self, state: &PrivilegeResetState) -> crate::SessionResult {
        self.updates.set(self.updates.get() + 1);
        let params = privilege_reset_completion_params(state);
        let mut current = self.current.borrow_mut();
        let current = current.as_mut().unwrap();
        assert_eq!(state.keyspace_name, current.name);
        for (key, expected) in params.Preconditions {
            if current.config.get(&key) != expected.as_ref() {
                return Err(crate::SessionError::new(
                    "keyspace config precondition failed",
                ));
            }
        }
        if self.fail_update.replace(false) {
            return Err(crate::SessionError::new("transient keyspace config update"));
        }
        for (key, value) in params.Config {
            current.config.insert(key, value.unwrap());
        }
        // PD update changes live metadata; a separate stale codec is explicitly installed below.
        *self.snapshot.borrow_mut() = current.clone();
        Ok(())
    }
}
fn reset_pd() -> ResetPd {
    let meta = StarterKeyspaceMeta {
        name: "restored_keyspace".into(),
        config: [
            (BRANCH_RESET_DONE_KEY.into(), "False".into()),
            (RESTORE_RESET_DONE_KEY.into(), "False".into()),
        ]
        .into(),
    };
    ResetPd {
        snapshot: std::cell::RefCell::new(meta.clone()),
        current: std::cell::RefCell::new(Some(meta)),
        loads: Default::default(),
        updates: Default::default(),
        fail_update: std::cell::Cell::new(true),
        fail_load: Default::default(),
    }
}
#[test]
fn starter_privilege_reset_workflow_retries_pd_cas_and_rechecks_stale_codec() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    seed_privileges(&session, "source_keyspace.user");
    update_starter_bootstrap_version(&session, 2).unwrap();
    finish_starter_bootstrap(&domain, 2).unwrap();
    let pd = reset_pd();
    let run = || {
        reconcile_starter_bootstrap_with_metadata(
            &domain,
            &reset_file(),
            "restored_keyspace",
            || Ok(()),
            Some(&pd),
        )
    };
    assert!(
        run()
            .unwrap_err()
            .to_string()
            .contains("transient keyspace config update")
    );
    assert_eq!(pd.updates.get(), 1);
    assert_eq!(get_store_starter_bootstrap_version(&domain).unwrap(), 3);
    assert_eq!(get_starter_bootstrap_version(&session).unwrap(), 3);
    assert_privileges(&session, "source_keyspace.user", 0);
    assert_eq!(
        scalar(
            &session,
            "SELECT COUNT(*) FROM mysql.user WHERE User='restored_keyspace.root' AND authentication_string=''"
        ),
        "1"
    );
    assert_eq!(
        pd.current.borrow().as_ref().unwrap().config[BRANCH_RESET_DONE_KEY],
        "False"
    );
    run().unwrap();
    assert_eq!(pd.updates.get(), 2);
    assert_eq!(
        pd.current.borrow().as_ref().unwrap().config[RESTORE_RESET_DONE_KEY],
        "True"
    );
    run().unwrap();
    assert_eq!(pd.updates.get(), 2);
    pd.snapshot
        .borrow_mut()
        .config
        .insert(RESTORE_RESET_DONE_KEY.into(), "False".into());
    run().unwrap();
    assert_eq!(pd.loads.get(), 3);
    assert_eq!(pd.updates.get(), 2);
    pd.snapshot
        .borrow_mut()
        .config
        .insert(RESTORE_RESET_DONE_KEY.into(), "invalid".into());
    seed_privileges(&session, "invalid_marker.user");
    assert!(
        run()
            .unwrap_err()
            .to_string()
            .contains("invalid starter privilege reset marker")
    );
    assert_privileges(&session, "invalid_marker.user", 1);
    domain.close();
}
#[test]
fn starter_privilege_reset_workflow_rejects_copied_newer_version_and_bad_pd_refresh() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    seed_privileges(&session, "source_keyspace.user");
    let pd = reset_pd();
    let run = || {
        reconcile_starter_bootstrap_with_metadata(
            &domain,
            &reset_file(),
            "restored_keyspace",
            || Ok(()),
            Some(&pd),
        )
    };
    pd.fail_load.set(true);
    assert!(
        run()
            .unwrap_err()
            .to_string()
            .contains("PD client is required")
    );
    pd.fail_load.set(false);
    let meta = pd.current.borrow_mut().take();
    assert!(
        run()
            .unwrap_err()
            .to_string()
            .contains("returned no keyspace")
    );
    *pd.current.borrow_mut() = meta;
    update_starter_bootstrap_version(&session, 4).unwrap();
    assert!(
        run()
            .unwrap_err()
            .to_string()
            .contains("older than copied version 4")
    );
    update_starter_bootstrap_version(&session, 2).unwrap();
    finish_starter_bootstrap(&domain, 5).unwrap();
    assert!(
        run()
            .unwrap_err()
            .to_string()
            .contains("older than copied version 5")
    );
    assert_privileges(&session, "source_keyspace.user", 1);
    assert_eq!(pd.updates.get(), 0);
    domain.close();
}
#[test]
fn starter_privilege_reset_http_uses_pd_patch_with_observed_preconditions() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut data = Vec::new();
        let mut buf = [0; 4096];
        let end;
        loop {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0);
            data.extend_from_slice(&buf[..n]);
            if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                end = index + 4;
                break;
            }
        }
        let header = String::from_utf8(data[..end].to_vec()).unwrap();
        assert!(header.starts_with("PATCH /pd/api/v2/keyspaces/restored_keyspace/config HTTP/1.1"));
        let length: usize = header
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|n| n.trim().parse().unwrap())
            })
            .unwrap();
        while data.len() < end + length {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0);
            data.extend_from_slice(&buf[..n]);
        }
        let body: serde_json::Value = serde_json::from_slice(&data[end..end + length]).unwrap();
        assert_eq!(body["config"][RESTORE_RESET_DONE_KEY], "True");
        assert_eq!(body["preconditions"][RESTORE_RESET_DONE_KEY], "False");
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 19\r\nConnection: close\r\n\r\n{\"state\":\"ENABLED\"}").unwrap();
    });
    let state = parse_privilege_reset(Some(StarterKeyspaceMeta {
        name: "restored_keyspace".into(),
        config: [(RESTORE_RESET_DONE_KEY.into(), "False".into())].into(),
    }))
    .unwrap();
    update_privilege_reset_config(&[endpoint], None, &state).unwrap();
    server.join().unwrap();
}

#[test]
fn starter_factory_requires_manifest_when_metadata_requests_privilege_reset() {
    use crate::runtime::CanonicalSessionFactory;
    let storage = std::sync::Arc::try_unwrap(
        astersql_store_mockstore_mockstorage::NewMockStorage(
            astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
            None,
        )
        .unwrap(),
    )
    .ok()
    .unwrap();
    let factory = CanonicalSessionFactory::from_storage_for_test(storage).unwrap();
    let pd = reset_pd();
    assert!(
        factory
            .reconcile_configured_starter_bootstrap(|| Ok(()), Some(&pd))
            .unwrap_err()
            .to_string()
            .contains("starter bootstrap file is required for pending privilege reset")
    );
    assert_eq!(pd.loads.get(), 0);
    assert_eq!(pd.updates.get(), 0);
    factory.domain().close();
}

#[test]
fn starter_privilege_reset_http_preserves_cas_failure_response() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut buffer = [0; 4096];
        let mut request = Vec::new();
        let end;
        loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                end = index + 4;
                break;
            }
        }
        let headers = String::from_utf8(request[..end].to_vec()).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse().unwrap())
            })
            .unwrap();
        while request.len() < end + length {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
        }
        let body = "keyspace config precondition failed";
        write!(stream, "HTTP/1.1 412 Precondition Failed\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let state = parse_privilege_reset(Some(StarterKeyspaceMeta {
        name: "restored_keyspace".into(),
        config: [(RESTORE_RESET_DONE_KEY.into(), "False".into())].into(),
    }))
    .unwrap();
    let error = update_privilege_reset_config(&[endpoint], None, &state).unwrap_err();
    assert!(error.to_string().contains("412 Precondition Failed"));
    assert!(
        error
            .to_string()
            .contains("keyspace config precondition failed")
    );
    server.join().unwrap();
}
