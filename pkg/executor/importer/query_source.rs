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

use astersql_infoschema::{CiString, DBInfo, InfoSchema, SchemaRef, Table};
use astersql_sessionctx_sessionstates::SessionStates;
use std::collections::BTreeMap;

/// Source metadata resolved by the submitter, including expanded views.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceTable {
    pub DBID: i64,
    pub ID: i64,
    pub Revision: u64,
}

/// Everything a query worker needs to reproduce the submitting SELECT.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct QuerySource {
    pub ReadTS: u64,
    pub SchemaVersion: i64,
    pub Timestamp: i64,
    pub Tables: Vec<SourceTable>,
    pub SessionStates: SessionStates,
}

/// Concrete sessions bind their existing setters/decoders here. No SQL or
/// transaction implementation is duplicated in the importer.
pub trait QuerySessionStates {
    fn Status(&self) -> u32;
    fn DecodeQueryStates(&self, states: &SessionStates) -> Result<(), String>;
}

pub const IMPORT_QUERY_WORKER_VARS: &[&str] = &[
    "tidb_snapshot",
    "tidb_read_staleness",
    "tidb_txn_read_ts",
    "tidb_enable_external_ts_read",
    "autocommit",
    "tidb_txn_mode",
    "tx_isolation",
    "transaction_isolation",
    "tx_read_only",
    "transaction_read_only",
    "tidb_mem_quota_query",
];

impl QuerySource {
    /// Copy before filtering; restoration never mutates the reusable capture.
    pub fn RestoreStates(&self, worker: &impl QuerySessionStates) -> Result<(), String> {
        let mut states = self.SessionStates.clone();
        states.SystemVars.retain(|name, _| {
            !IMPORT_QUERY_WORKER_VARS.contains(&name.as_str())
                && !astersql_sessionctx_variable::GetSysVar(name)
                    .is_some_and(|var| var.InternalSessionVariable)
        });
        states.Status = worker.Status();
        states.Warnings.clear();
        worker.DecodeQueryStates(&states)
    }

    /// Rebuild from real MVCC metadata, not the current catalog. Preserve the
    /// submitter's schema version while checking every resolved table revision.
    pub fn BuildMinimalInfoSchema(
        &self,
        store: &dyn astersql_kv::Storage,
        parent: &dyn InfoSchema,
    ) -> Result<SchemaRef, String> {
        self.BuildMinimalInfoSchemaWithCharsetPolicy(store, parent, true)
    }

    /// Go defaults TreatOldVersionUTF8AsUTF8MB4 to true. An embedding with a
    /// nondefault compatibility policy can pass it explicitly without altering
    /// the immutable snapshot or duplicating metadata loading.
    pub fn BuildMinimalInfoSchemaWithCharsetPolicy(
        &self,
        store: &dyn astersql_kv::Storage,
        parent: &dyn InfoSchema,
        treat_old_utf8_as_utf8mb4: bool,
    ) -> Result<SchemaRef, String> {
        let reader = astersql_meta::SnapshotReader::new(
            store.GetSnapshot(astersql_kv::NewVersion(self.ReadTS)),
        );
        let mut databases: BTreeMap<i64, DBInfo> = BTreeMap::new();
        for src in &self.Tables {
            if !databases.contains_key(&src.DBID) {
                let db = reader
                    .get_database(src.DBID)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| {
                        format!(
                            "source database {} not found at the import query snapshot",
                            src.DBID
                        )
                    })?;
                databases.insert(
                    src.DBID,
                    DBInfo {
                        id: db.ID,
                        name: CiString::new(db.Name.O),
                        ..Default::default()
                    },
                );
            }
            let mut table = reader
                .get_table(src.DBID, src.ID)
                .map_err(|e| e.to_string())?
                .filter(|table| table.Revision == src.Revision)
                .ok_or_else(|| {
                    format!(
                        "source table {} changed before the import query snapshot",
                        src.ID
                    )
                })?;
            // Preserve the order of Go's legacy conversions: upgrade old utf8
            // first, then lowercase pre-v3 charset/collation metadata.
            if table.Version < astersql_meta_model::TableInfoVersion2 && treat_old_utf8_as_utf8mb4 {
                if table.Charset == "utf8" {
                    table.Charset = "utf8mb4".into();
                    table.Collate = "utf8mb4_bin".into();
                }
                for column in &mut table.Columns {
                    if column.Version < astersql_meta_model::ColumnInfoVersion2
                        && column.GetCharset() == "utf8"
                    {
                        column.SetCharset("utf8mb4".into());
                        column.SetCollate("utf8mb4_bin".into());
                    }
                }
            }
            if table.Version < astersql_meta_model::TableInfoVersion3 {
                table.Charset = table.Charset.to_ascii_lowercase();
                table.Collate = table.Collate.to_ascii_lowercase();
                for column in &mut table.Columns {
                    column.SetCharset(column.GetCharset().to_ascii_lowercase());
                    column.SetCollate(column.GetCollate().to_ascii_lowercase());
                }
            }
            table.DBID = src.DBID;
            table.TableCacheStatusType = astersql_meta_model::TableCacheStatusDisable;
            databases
                .get_mut(&src.DBID)
                .unwrap()
                .tables
                .push(Table::from_model(table).0);
        }
        let mut dbs: Vec<_> = databases.into_values().collect();
        let mut builder = astersql_infoschema::builder::NewBuilder(0, Default::default(), false)
            .WithCrossKS(true);
        builder.InitWithDBInfos(
            &mut dbs,
            parent
                .AllPlacementPolicies()
                .into_iter()
                .map(|p| (*p).clone())
                .collect(),
            Vec::new(),
            self.SchemaVersion,
        );
        Ok(builder.Build(self.ReadTS))
    }
}
