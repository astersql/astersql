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

//! Starter-only versioned SQL manifest, reconciled independently of core bootstrap.

use crate::runtime::{ConcreteSession, quote_argument};
use crate::{SessionError, SessionResult};
use astersql_domain::Domain;
use astersql_kv as kv;
use astersql_util_logutil::log::{BgLogger, LogField, LogLevel};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

pub const VERSION_VAR: &str = "starter_bootstrap_version";
const VERSION_COMMENT: &str = "Starter bootstrap file version. Do not delete.";

#[derive(Debug, Default)]
pub struct StarterBootstrapFile {
    pub version: i64,
    pub bootstrap: Vec<String>,
    pub upgrades: Vec<StarterBootstrapUpgrade>,
}
#[derive(Debug, Default)]
pub struct StarterBootstrapUpgrade {
    pub version: i64,
    pub sql: Vec<String>,
}

// Go's JSON struct decoder accepts case-insensitive field names, last duplicate
// fields, and null slices/scalars. Keep those semantics with unknown-field checks.
impl<'de> Deserialize<'de> for StarterBootstrapFile {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct FileVisitor;
        impl<'de> serde::de::Visitor<'de> for FileVisitor {
            type Value = StarterBootstrapFile;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a starter bootstrap JSON object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut file = StarterBootstrapFile::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.to_ascii_lowercase().as_str() {
                        "version" => {
                            if let Some(version) = map.next_value::<Option<i64>>()? {
                                file.version = version;
                            }
                        }
                        "bootstrap" => {
                            file.bootstrap = map
                                .next_value::<Option<Vec<Option<String>>>>()?
                                .unwrap_or_default()
                                .into_iter()
                                .map(Option::unwrap_or_default)
                                .collect()
                        }
                        "upgrades" => {
                            file.upgrades = map
                                .next_value::<Option<Vec<Option<StarterBootstrapUpgrade>>>>()?
                                .unwrap_or_default()
                                .into_iter()
                                .map(Option::unwrap_or_default)
                                .collect()
                        }
                        _ => {
                            return Err(serde::de::Error::custom(format!("unknown field {key:?}")));
                        }
                    }
                }
                Ok(file)
            }
        }
        decoder.deserialize_map(FileVisitor)
    }
}
impl<'de> Deserialize<'de> for StarterBootstrapUpgrade {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct UpgradeVisitor;
        impl<'de> serde::de::Visitor<'de> for UpgradeVisitor {
            type Value = StarterBootstrapUpgrade;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a starter bootstrap upgrade JSON object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut upgrade = StarterBootstrapUpgrade::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.to_ascii_lowercase().as_str() {
                        "version" => {
                            if let Some(version) = map.next_value::<Option<i64>>()? {
                                upgrade.version = version;
                            }
                        }
                        "sql" => {
                            upgrade.sql = map
                                .next_value::<Option<Vec<Option<String>>>>()?
                                .unwrap_or_default()
                                .into_iter()
                                .map(Option::unwrap_or_default)
                                .collect()
                        }
                        _ => {
                            return Err(serde::de::Error::custom(format!("unknown field {key:?}")));
                        }
                    }
                }
                Ok(upgrade)
            }
        }
        decoder.deserialize_map(UpgradeVisitor)
    }
}

pub fn parse_starter_bootstrap_file(data: &[u8]) -> SessionResult<StarterBootstrapFile> {
    let mut decoder = serde_json::Deserializer::from_slice(data);
    let mut file = Option::<StarterBootstrapFile>::deserialize(&mut decoder)
        .map_err(|e| SessionError::new(e.to_string().replace('`', "\"")))?
        .unwrap_or_default();
    decoder.end().map_err(|e| {
        SessionError::new(format!(
            "bootstrap file must contain a single JSON object: {e}"
        ))
    })?;
    if file.version <= 0 {
        return Err(SessionError::new(
            "bootstrap file version must be greater than 0",
        ));
    }
    validate_blocks("bootstrap", &file.bootstrap)?;
    let mut versions = HashSet::new();
    for (i, upgrade) in file.upgrades.iter().enumerate() {
        if upgrade.version <= 0 {
            return Err(SessionError::new(format!(
                "upgrades[{i}].version must be greater than 0"
            )));
        }
        if upgrade.version > file.version {
            return Err(SessionError::new(format!(
                "upgrades[{i}].version {} is greater than bootstrap file version {}",
                upgrade.version, file.version
            )));
        }
        if !versions.insert(upgrade.version) {
            return Err(SessionError::new(format!(
                "duplicated upgrade version {}",
                upgrade.version
            )));
        }
        validate_blocks(&format!("upgrades[{i}].sql"), &upgrade.sql)?;
    }
    file.upgrades.sort_by_key(|u| u.version);
    Ok(file)
}
fn validate_blocks(field: &str, blocks: &[String]) -> SessionResult {
    static PLACEHOLDERS: OnceLock<regex::Regex> = OnceLock::new();
    let placeholders = PLACEHOLDERS.get_or_init(|| {
        regex::Regex::new(r"<[A-Za-z0-9_-]+>").expect("constant placeholder regex")
    });
    for (i, block) in blocks.iter().enumerate() {
        if block.trim().is_empty() {
            return Err(SessionError::new(format!("{field}[{i}] must not be empty")));
        }
        for placeholder in placeholders.find_iter(block) {
            if placeholder.as_str() != "<keyspace>" {
                return Err(SessionError::new(format!(
                    "{field}[{i}] uses unsupported placeholder {:?}",
                    placeholder.as_str()
                )));
            }
        }
    }
    Ok(())
}
impl StarterBootstrapFile {
    pub fn pending_upgrades(&self, stored_version: i64) -> &[StarterBootstrapUpgrade] {
        &self.upgrades[self
            .upgrades
            .partition_point(|u| u.version <= stored_version)..]
    }
    pub fn needs_upgrade(&self, stored_version: i64) -> bool {
        if stored_version > self.version {
            BgLogger().log(
                LogLevel::Warn,
                "starter bootstrap file is older than cluster state",
                [
                    LogField::I64("storedVersion".into(), stored_version),
                    LogField::I64("bootstrapFileVersion".into(), self.version),
                ],
            );
        }
        stored_version < self.version
    }
}
pub fn render_starter_bootstrap_sql(sql: &str, keyspace: &str) -> String {
    sql.replace(
        "<keyspace>",
        &astersql_util_sqlescape::EscapeString(keyspace),
    )
}
pub fn load_starter_bootstrap_file() -> SessionResult<Option<StarterBootstrapFile>> {
    if !astersql_config_deploymode::IsStarter() {
        return Ok(None);
    }
    let path = astersql_config::get_global_config()
        .starter_params
        .bootstrap_file
        .clone();
    if path.is_empty() {
        return Ok(None);
    }
    let data = std::fs::read(&path)
        .map_err(|e| SessionError::new(format!("read starter bootstrap file {path}: {e}")))?;
    let file = parse_starter_bootstrap_file(&data)
        .map_err(|e| SessionError::new(format!("parse starter bootstrap file {path}: {e}")))?;
    BgLogger().log(
        LogLevel::Info,
        "loaded starter bootstrap file",
        [
            LogField::String("file".into(), path),
            LogField::I64("version".into(), file.version),
            LogField::U64("bootstrapBlocks".into(), file.bootstrap.len() as u64),
            LogField::U64("upgradeEntries".into(), file.upgrades.len() as u64),
        ],
    );
    Ok(Some(file))
}
fn execute_and_close(session: &ConcreteSession, sql: &str) -> SessionResult {
    for mut records in session.execute(sql)? {
        records.close()?;
    }
    Ok(())
}
pub fn execute_starter_bootstrap_sql_blocks(
    session: &ConcreteSession,
    blocks: &[String],
    keyspace: &str,
) -> SessionResult {
    if blocks.is_empty() {
        return Ok(());
    }
    session.with_starter_restricted_sql(|| {
        for (i, block) in blocks.iter().enumerate() {
            let sql = render_starter_bootstrap_sql(block, keyspace);
            let statement_count = session
                .starter_statement_count(&sql)
                .map_err(|e| SessionError::new(format!("parse SQL block {i}: {e}")))?;
            if statement_count != 1 {
                return Err(SessionError::new(format!(
                    "SQL block {i} must contain exactly one statement"
                )));
            }
            let records = session
                .execute(&sql)
                .map_err(|e| SessionError::new(format!("execute SQL block {i}: {e}")))?;
            for mut records in records {
                records
                    .close()
                    .map_err(|e| SessionError::new(format!("close SQL result: {e}")))?;
            }
        }
        Ok(())
    })
}
pub fn get_starter_bootstrap_version(session: &ConcreteSession) -> SessionResult<i64> {
    let mut records = session.execute(&format!(
        "SELECT VARIABLE_VALUE FROM mysql.tidb WHERE VARIABLE_NAME={}",
        quote_argument(VERSION_VAR)
    ))?;
    let record = records
        .first_mut()
        .ok_or_else(|| SessionError::new("starter version query returned no result"))?;
    let row_result = record.next_row();
    let close_result = record.close();
    let row = row_result?;
    close_result?;
    let Some(row) = row else {
        return Ok(0);
    };
    let value = row
        .first()
        .ok_or_else(|| SessionError::new("starter version row returned no value"))?;
    value
        .parse()
        .map_err(|e| SessionError::new(format!("invalid starter bootstrap version {value:?}: {e}")))
}
pub fn update_starter_bootstrap_version(session: &ConcreteSession, version: i64) -> SessionResult {
    session.with_starter_restricted_sql(|| execute_and_close(session, &format!("INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ({}, {}, {}) ON DUPLICATE KEY UPDATE VARIABLE_VALUE={}", quote_argument(VERSION_VAR), quote_argument(&version.to_string()), quote_argument(VERSION_COMMENT), quote_argument(&version.to_string()))))
}
pub const PRIVILEGE_RESET_TABLES: [&str; 8] = [
    "columns_priv",
    "db",
    "default_roles",
    "global_grants",
    "global_priv",
    "role_edges",
    "tables_priv",
    "user",
];
const PRIVILEGE_RESET_BATCH_SIZE: u64 = 128;

fn prepare_bootstrap_stmts(
    session: &ConcreteSession,
    blocks: &[String],
    keyspace: &str,
) -> SessionResult<Vec<String>> {
    if blocks.is_empty() {
        return Err(SessionError::new(
            "starter bootstrap file must contain bootstrap SQL",
        ));
    }
    session.with_starter_restricted_sql(|| {
        let mut stmts = Vec::with_capacity(blocks.len());
        // Parse every block before validating statement types or deleting rows.
        for (i, block) in blocks.iter().enumerate() {
            let sql = render_starter_bootstrap_sql(block, keyspace);
            let count = session
                .starter_statement_count(&sql)
                .map_err(|e| SessionError::new(format!("parse SQL block {i}: {e}")))?;
            if count != 1 {
                return Err(SessionError::new(format!(
                    "SQL block {i} must contain exactly one statement"
                )));
            }
            stmts.push(sql);
        }
        for (i, sql) in stmts.iter().enumerate() {
            if !session.validate_starter_bootstrap_statement(sql)? {
                return Err(SessionError::new(format!(
                    "bootstrap SQL block {i} must be INSERT, REPLACE, UPDATE, or DELETE"
                )));
            }
        }
        Ok(stmts)
    })
}
fn execute_bootstrap_stmts(session: &ConcreteSession, stmts: &[String]) -> SessionResult {
    session.with_starter_restricted_sql(|| {
        for (i, sql) in stmts.iter().enumerate() {
            let records = session
                .execute(sql)
                .map_err(|e| SessionError::new(format!("execute SQL block {i}: {e}")))?;
            for mut record in records {
                record
                    .close()
                    .map_err(|e| SessionError::new(format!("close SQL result: {e}")))?;
            }
        }
        Ok(())
    })
}
fn verify_root_user(session: &ConcreteSession, keyspace: &str) -> SessionResult {
    let root = format!("{keyspace}.root");
    let mut records = session
        .execute(&format!(
            "SELECT 1 FROM mysql.user WHERE Host = '%' AND User = {} LIMIT 1",
            quote_argument(&root)
        ))
        .map_err(|e| SessionError::new(format!("verify starter root user: {e}")))?;
    let record = records
        .first_mut()
        .ok_or_else(|| SessionError::new("verify starter root user returned no result"))?;
    let row = record.next_row();
    let closed = record.close();
    let row = row.map_err(|e| SessionError::new(format!("verify starter root user: {e}")))?;
    closed
        .map_err(|e| SessionError::new(format!("close starter root verification result: {e}")))?;
    if row.is_none() {
        return Err(SessionError::new(format!(
            "starter bootstrap file must create '{root}'@'%'"
        )));
    }
    Ok(())
}
pub fn run_starter_bootstrap_locked(
    session: &ConcreteSession,
    file: &StarterBootstrapFile,
    keyspace: &str,
) -> SessionResult {
    let stmts = prepare_bootstrap_stmts(session, &file.bootstrap, keyspace)?;
    run_bootstrap_txn(session, file, &stmts, keyspace)
}
pub fn reset_privileges_locked(
    session: &ConcreteSession,
    file: &StarterBootstrapFile,
    keyspace: &str,
) -> SessionResult {
    let stmts = prepare_bootstrap_stmts(session, &file.bootstrap, keyspace)?;
    for table in PRIVILEGE_RESET_TABLES {
        loop {
            let affected = delete_privilege_batch(session, table).map_err(|e| {
                SessionError::new(format!("reset starter privilege table mysql.{table}: {e}"))
            })?;
            if affected < PRIVILEGE_RESET_BATCH_SIZE {
                break;
            }
        }
    }
    run_bootstrap_txn(session, file, &stmts, keyspace)
}
fn delete_privilege_batch(session: &ConcreteSession, table: &str) -> SessionResult<u64> {
    execute_and_close(session, "BEGIN")?;
    let result = (|| {
        execute_and_close(
            session,
            &format!("DELETE FROM mysql.`{table}` LIMIT {PRIVILEGE_RESET_BATCH_SIZE}"),
        )?;
        let affected = session.protocol_state().affected_rows;
        execute_and_close(session, "COMMIT")?;
        Ok(affected)
    })();
    if result.is_err() {
        if let Err(error) = execute_and_close(session, "ROLLBACK") {
            BgLogger().log(
                LogLevel::Warn,
                "rollback starter privilege reset batch failed",
                [LogField::String("error".into(), error.to_string())],
            );
        }
    }
    result
}
fn run_bootstrap_txn(
    session: &ConcreteSession,
    file: &StarterBootstrapFile,
    stmts: &[String],
    keyspace: &str,
) -> SessionResult {
    execute_and_close(session, "BEGIN")
        .map_err(|e| SessionError::new(format!("begin starter bootstrap file: {e}")))?;
    let result = (|| {
        execute_bootstrap_stmts(session, stmts)?;
        verify_root_user(session, keyspace)?;
        update_starter_bootstrap_version(session, file.version)?;
        execute_and_close(session, "COMMIT")
            .map_err(|e| SessionError::new(format!("commit starter bootstrap file: {e}")))
    })();
    if result.is_err() {
        if let Err(error) = execute_and_close(session, "ROLLBACK") {
            astersql_util_logutil::log::BgLogger().log(
                astersql_util_logutil::log::LogLevel::Warn,
                "rollback starter bootstrap file failed",
                [astersql_util_logutil::log::LogField::String(
                    "error".into(),
                    error.to_string(),
                )],
            );
        }
    }
    result
}
pub fn upgrade_starter_bootstrap_from_version(
    session: &ConcreteSession,
    file: &StarterBootstrapFile,
    stored_version: i64,
    keyspace: &str,
) -> SessionResult {
    if !file.needs_upgrade(stored_version) {
        return Ok(());
    }
    for upgrade in file.pending_upgrades(stored_version) {
        BgLogger().log(
            LogLevel::Info,
            "running starter bootstrap file upgrade",
            [
                LogField::I64("storedVersion".into(), stored_version),
                LogField::I64("upgradeVersion".into(), upgrade.version),
                LogField::I64("targetVersion".into(), file.version),
            ],
        );
        execute_starter_bootstrap_sql_blocks(session, &upgrade.sql, keyspace).map_err(|e| {
            SessionError::new(format!(
                "upgrade starter bootstrap file to version {}: {e}",
                upgrade.version
            ))
        })?;
    }
    update_starter_bootstrap_version(session, file.version)
}
pub fn get_store_starter_bootstrap_version(domain: &Arc<Domain>) -> SessionResult<i64> {
    let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnBootstrap);
    let mut version = 0;
    domain
        .storage_handle()
        .with_storage(|store| {
            kv::RunInNewTxn(&context, store, false, |_, txn| {
                match txn.Get(
                    &context,
                    astersql_meta::transaction_meta_string_key(b"StarterBootstrapKey"),
                    &[],
                ) {
                    Ok(value) if value.Value.is_empty() => Ok(()),
                    Ok(value) => {
                        version = std::str::from_utf8(&value.Value)
                            .map_err(|e| kv::errors::New(e.to_string()))?
                            .parse()
                            .map_err(|e: std::num::ParseIntError| kv::errors::New(e.to_string()))?;
                        Ok(())
                    }
                    Err(e) if kv::IsErrNotFound(&e) => Ok(()),
                    Err(e) => Err(e),
                }
            })
        })
        .map_err(|e| SessionError::new(format!("get starter bootstrap version from store: {e}")))?;
    Ok(version)
}
pub fn finish_starter_bootstrap(domain: &Arc<Domain>, version: i64) -> SessionResult {
    let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnBootstrap);
    domain
        .storage_handle()
        .with_storage(|store| {
            kv::RunInNewTxn(&context, store, true, |_, txn| {
                // Apply the same priority/disk-full options as Go meta.NewMutator.
                let _ = astersql_meta::TransactionMutator::new(txn);
                txn.Set(
                    astersql_meta::transaction_meta_string_key(b"StarterBootstrapKey"),
                    version.to_string().into_bytes(),
                )
            })
        })
        .map_err(|e| SessionError::new(format!("finish starter bootstrap in store: {e}")))
}
/// The completion-key fast path avoids constructing another session/domain. The
/// caller supplies the same namespaced owner lock used by core bootstrap.
pub fn reconcile_starter_bootstrap<G>(
    domain: &Arc<Domain>,
    file: &StarterBootstrapFile,
    keyspace: &str,
    acquire: impl FnOnce() -> SessionResult<G>,
) -> SessionResult {
    reconcile_starter_bootstrap_with_metadata(domain, file, keyspace, acquire, None)
}
pub fn reconcile_starter_bootstrap_with_metadata<G>(
    domain: &Arc<Domain>,
    file: &StarterBootstrapFile,
    keyspace: &str,
    acquire: impl FnOnce() -> SessionResult<G>,
    metadata: Option<&dyn StarterPrivilegeResetMetadata>,
) -> SessionResult {
    let mut reset = parse_privilege_reset(metadata.map(|m| m.snapshot()).transpose()?.flatten())?;
    if reset.pending_markers.is_empty()
        && !file.needs_upgrade(get_store_starter_bootstrap_version(domain)?)
    {
        return Ok(());
    }
    let started = std::time::Instant::now();
    let _guard = acquire().map_err(|e| {
        SessionError::new(format!("acquire starter bootstrap file upgrade lock: {e}"))
    })?;
    if !reset.pending_markers.is_empty() {
        let refreshed = metadata
            .expect("pending reset requires snapshot provider")
            .refresh(&reset.keyspace_name)?
            .ok_or_else(|| {
                SessionError::new("refresh starter privilege reset metadata returned no keyspace")
            })?;
        reset = parse_privilege_reset(Some(refreshed))?;
    }
    if reset.pending_markers.is_empty()
        && !file.needs_upgrade(get_store_starter_bootstrap_version(domain)?)
    {
        return Ok(());
    }
    let session = ConcreteSession::new(domain.clone());
    session.set_starter_clustered_index_mode();
    let version = get_starter_bootstrap_version(&session)?;
    if !reset.pending_markers.is_empty() {
        let copied_version = get_store_starter_bootstrap_version(domain)?.max(version);
        if copied_version > file.version {
            return Err(SessionError::new(format!(
                "starter bootstrap file version {} is older than copied version {copied_version}",
                file.version
            )));
        }
        reset_privileges_locked(&session, file, keyspace)?;
        finish_starter_bootstrap(domain, file.version)?;
        metadata
            .expect("pending reset requires snapshot provider")
            .complete(&reset)
            .map_err(|e| SessionError::new(format!("complete starter privilege reset: {e}")))?;
        BgLogger().log(
            LogLevel::Info,
            "starter privilege reset finished",
            [
                LogField::String("keyspace".into(), reset.keyspace_name),
                LogField::I64("version".into(), file.version),
                LogField::String("cost".into(), format!("{:?}", started.elapsed())),
            ],
        );
        return Ok(());
    }
    if !file.needs_upgrade(version) {
        return finish_starter_bootstrap(domain, version);
    }
    if version == 0 {
        run_starter_bootstrap_locked(&session, file, keyspace)?;
    } else {
        upgrade_starter_bootstrap_from_version(&session, file, version, keyspace)?;
    }
    finish_starter_bootstrap(domain, file.version)?;
    BgLogger().log(
        LogLevel::Info,
        if version == 0 {
            "starter bootstrap file initialization finished"
        } else {
            "starter bootstrap file upgrade finished"
        },
        [
            LogField::I64("version".into(), file.version),
            LogField::String("cost".into(), format!("{:?}", started.elapsed())),
        ],
    );
    Ok(())
}
pub const BRANCH_RESET_DONE_KEY: &str = "serverless_is_branch_bootstrapped";
pub const RESTORE_RESET_DONE_KEY: &str = "serverless_is_bootstrapped_for_restore";
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrivilegeResetState {
    pub keyspace_name: String,
    pub pending_markers: std::collections::HashMap<String, String>,
}
#[derive(Clone, Debug, Default)]
pub struct StarterKeyspaceMeta {
    pub name: String,
    pub config: std::collections::HashMap<String, String>,
}
/// Storage codec snapshot and PD refresh/update boundaries. SQL/KV execution
/// remains in the canonical session, including retries after a failed PD CAS.
pub trait StarterPrivilegeResetMetadata {
    fn snapshot(&self) -> SessionResult<Option<StarterKeyspaceMeta>>;
    fn refresh(&self, name: &str) -> SessionResult<Option<StarterKeyspaceMeta>>;
    fn complete(&self, state: &PrivilegeResetState) -> SessionResult;
}
pub fn parse_privilege_reset(
    meta: Option<StarterKeyspaceMeta>,
) -> SessionResult<PrivilegeResetState> {
    let Some(meta) = meta else {
        return Ok(PrivilegeResetState::default());
    };
    let mut state = PrivilegeResetState {
        keyspace_name: meta.name,
        ..Default::default()
    };
    for key in [BRANCH_RESET_DONE_KEY, RESTORE_RESET_DONE_KEY] {
        let Some(value) = meta.config.get(key).filter(|v| !v.is_empty()) else {
            continue;
        };
        match value.as_str() {
            "1" | "t" | "T" | "TRUE" | "true" | "True" => {}
            "0" | "f" | "F" | "FALSE" | "false" | "False" => {
                state.pending_markers.insert(key.into(), value.clone());
            }
            _ => {
                return Err(SessionError::new(format!(
                    "invalid starter privilege reset marker {key}={value:?}"
                )));
            }
        }
    }
    Ok(state)
}
pub fn privilege_reset_completion_params(
    state: &PrivilegeResetState,
) -> astersql_domain_infosync::UpdateKeyspaceConfigParams {
    astersql_domain_infosync::UpdateKeyspaceConfigParams {
        Config: state
            .pending_markers
            .keys()
            .map(|key| (key.clone(), Some("True".into())))
            .collect(),
        Preconditions: state
            .pending_markers
            .iter()
            .map(|(key, value)| (key.clone(), Some(value.clone())))
            .collect(),
    }
}

pub(crate) fn update_privilege_reset_config(
    endpoints: &[String],
    tls: Option<(String, String, String)>,
    state: &PrivilegeResetState,
) -> SessionResult {
    let mut builder =
        reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(10));
    let scheme = if let Some((ca, cert, key)) = tls {
        let ca = std::fs::read(ca).map_err(|e| SessionError::new(e.to_string()))?;
        let mut identity = std::fs::read(cert).map_err(|e| SessionError::new(e.to_string()))?;
        identity.extend(std::fs::read(key).map_err(|e| SessionError::new(e.to_string()))?);
        builder = builder
            .add_root_certificate(
                reqwest::Certificate::from_pem(&ca)
                    .map_err(|e| SessionError::new(e.to_string()))?,
            )
            .identity(
                reqwest::Identity::from_pem(&identity)
                    .map_err(|e| SessionError::new(e.to_string()))?,
            );
        "https"
    } else {
        "http"
    };
    let client = builder
        .build()
        .map_err(|e| SessionError::new(e.to_string()))?;
    let params = privilege_reset_completion_params(state);
    let mut last_error =
        SessionError::new("PD HTTP client is required to complete starter privilege reset");
    for endpoint in endpoints {
        let endpoint = if endpoint.contains("://") {
            endpoint.clone()
        } else {
            format!("{scheme}://{endpoint}")
        };
        let mut url = url::Url::parse(&endpoint).map_err(|e| SessionError::new(e.to_string()))?;
        url.path_segments_mut()
            .map_err(|_| SessionError::new("invalid PD HTTP endpoint"))?
            .clear()
            .extend([
                "pd",
                "api",
                "v2",
                "keyspaces",
                state.keyspace_name.as_str(),
                "config",
            ]);
        match client.patch(url).json(&params).send() {
            Ok(response) => {
                let status = response.status();
                if !status.is_success() {
                    let body = response
                        .text()
                        .map_err(|e| SessionError::new(e.to_string()))?;
                    last_error = SessionError::new(format!(
                        "PD keyspace config update returned {status}: {body}"
                    ));
                    continue;
                }
                // Consume/validate the response rather than acknowledge a malformed PD reply.
                let meta = response
                    .json::<serde_json::Value>()
                    .map_err(|e| SessionError::new(e.to_string()))?;
                if !matches!(
                    meta.get("state").and_then(serde_json::Value::as_str),
                    Some("ENABLED" | "DISABLED" | "ARCHIVED" | "TOMBSTONE")
                ) {
                    return Err(SessionError::new(
                        "invalid PD keyspace state in privilege reset response",
                    ));
                }
                return Ok(());
            }
            Err(error) => last_error = SessionError::new(error.to_string()),
        }
    }
    Err(last_error)
}
