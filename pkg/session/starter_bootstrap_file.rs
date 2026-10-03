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
pub fn run_starter_bootstrap_locked(
    session: &ConcreteSession,
    file: &StarterBootstrapFile,
    keyspace: &str,
) -> SessionResult {
    execute_and_close(session, "BEGIN")
        .map_err(|e| SessionError::new(format!("begin starter bootstrap file: {e}")))?;
    let result = (|| {
        execute_starter_bootstrap_sql_blocks(session, &file.bootstrap, keyspace)?;
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
    if !file.needs_upgrade(get_store_starter_bootstrap_version(domain)?) {
        return Ok(());
    }
    let started = std::time::Instant::now();
    let _guard = acquire().map_err(|e| {
        SessionError::new(format!("acquire starter bootstrap file upgrade lock: {e}"))
    })?;
    if !file.needs_upgrade(get_store_starter_bootstrap_version(domain)?) {
        return Ok(());
    }
    let session = ConcreteSession::new(domain.clone());
    session.set_starter_clustered_index_mode();
    let version = get_starter_bootstrap_version(&session)?;
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
