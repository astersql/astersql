// Copyright 2026 AsterSQL.

//! Canonical Domain-backed Extract worker and status HTTP runtime.

use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use astersql_domain::Domain;
use astersql_domain::extract::{
    ExtractHandle, ExtractPlanPackage, ExtractSource, ExtractTask as DomainExtractTask,
    StatementRecord, TableNamePair,
};
use astersql_domain::plan_replayer_dump::{ReplayArchive, encode_replay_archive};
use astersql_infoschema::InfoSchema as _;
use astersql_planner_extstore::{Context as StorageContext, GetGlobalExtStorage};
use astersql_server_handler_extractorhandler::extractor::{
    ExtractError, ExtractReader, ExtractResult, ExtractRuntime, ExtractTask, RequestContext,
    Timestamp,
};
use astersql_session::runtime::ConcreteSession;
use astersql_util_stmtsummary::StmtSummaryByDigestMap;

const EXTRACT_DIRECTORY: &str = "extract";

fn sql_identifier(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

fn sql_rows(domain: &Arc<Domain>, sql: &str) -> Result<Vec<Vec<String>>, String> {
    let session = ConcreteSession::new(Arc::clone(domain));
    let mut rows = Vec::new();
    for mut record_set in session.execute(sql).map_err(|error| error.to_string())? {
        while let Some(row) = record_set.next_row().map_err(|error| error.to_string())? {
            rows.push(row);
        }
        record_set.close().map_err(|error| error.to_string())?;
    }
    Ok(rows)
}

struct ProductionExtractSource {
    domain: Arc<Domain>,
}

impl ExtractSource for ProductionExtractSource {
    fn statement_records(&self, task: &DomainExtractTask) -> Result<Vec<StatementRecord>, String> {
        let begin = task
            .begin
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let end = task
            .end
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let summaries = StmtSummaryByDigestMap
            .lock()
            .map_err(|_| "statement summary lock poisoned".to_owned())?
            .Summaries();
        let mut records = Vec::new();
        for summary in summaries {
            let windows = summary
                .history
                .iter()
                .enumerate()
                .filter(|(index, window)| {
                    (task.use_history_view || *index + 1 == summary.history.len())
                        && window.endTime > begin
                        && window.beginTime < end
                });
            for (_, window) in windows {
                let tables = summary
                    .tableNames
                    .split(',')
                    .filter_map(|name| name.trim().split_once('.'))
                    .map(|(database, table)| TableNamePair {
                        database: database.to_owned(),
                        table: table.to_owned(),
                        is_view: false,
                    })
                    .collect();
                records.push(StatementRecord {
                    statement_type: summary.stmtType.clone(),
                    schema_name: summary.schemaName.clone(),
                    tables,
                    digest: summary.digest.clone(),
                    plan_digest: summary.planDigest.clone(),
                    sql: window.stmtSummaryStats.sampleSQL.clone(),
                    binary_plan: window.stmtSummaryStats.sampleBinaryPlan.clone(),
                    user_name: window
                        .stmtSummaryStats
                        .authUsers
                        .iter()
                        .next()
                        .cloned()
                        .unwrap_or_default(),
                    decoded_plan: String::new(),
                    skipped: false,
                });
            }
        }
        Ok(records)
    }

    fn table(&self, database: &str, table: &str) -> Result<Option<TableNamePair>, String> {
        Ok(self
            .domain
            .stats_table(database, table)
            .map(|(_, info)| TableNamePair {
                database: database.to_owned(),
                table: table.to_owned(),
                is_view: info.View.is_some(),
            }))
    }

    fn view_dependencies(&self, _view: &TableNamePair) -> Result<Vec<TableNamePair>, String> {
        Err("view dependency lookup requires the Domain AST wrapper".to_owned())
    }

    fn decode_binary_plan(&self, encoded: &str) -> Result<String, String> {
        // The SQL builtin returns an empty string and appends a warning when
        // the plan payload cannot be decoded. The Extract worker only reads
        // the returned string, so use the same codec directly here.
        Ok(astersql_util_plancodec::DecodeBinaryPlan(encoded).unwrap_or_default())
    }

    fn dump_package(
        &self,
        file_name: &str,
        task: &DomainExtractTask,
        package: &ExtractPlanPackage,
    ) -> Result<(), String> {
        let mut archive = ReplayArchive::default();
        archive.write(
            "extract_meta.txt",
            format!("SkipStats = \"{}\"\ntaskType = \"Plan\"\n", task.skip_stats),
        )?;
        archive.write(
            "meta.txt",
            format!("AsterSQL {}\n", env!("CARGO_PKG_VERSION")),
        )?;
        archive.write(
            "config.toml",
            toml::to_string(&*astersql_config::config::get_global_config())
                .map_err(|error| error.to_string())?,
        )?;
        archive.write(
            "schema/schema_meta.txt",
            format!(
                "schema_version = {}\n",
                self.domain.info_schema().SchemaMetaVersion()
            ),
        )?;
        let variables = sql_rows(&self.domain, "SHOW VARIABLES")?;
        archive.write(
            "variables.toml",
            variables
                .into_iter()
                .filter_map(|row| (row.len() >= 2).then(|| format!("{} = {:?}\n", row[0], row[1])))
                .collect::<String>(),
        )?;
        let bindings = sql_rows(&self.domain, "SHOW GLOBAL BINDINGS")?;
        archive.write(
            "bindings.sql",
            bindings
                .into_iter()
                .filter_map(|row| row.first().cloned())
                .collect::<Vec<_>>()
                .join(";\n"),
        )?;
        let mut replicas = String::new();
        for table in &package.tables {
            let qualified = format!(
                "{}.{}",
                sql_identifier(&table.database),
                sql_identifier(&table.table)
            );
            let create = sql_rows(&self.domain, &format!("SHOW CREATE TABLE {qualified}"))?
                .into_iter()
                .next()
                .and_then(|row| row.get(1).cloned())
                .ok_or_else(|| format!("SHOW CREATE returned no definition for {qualified}"))?;
            let path = if table.is_view { "view" } else { "schema" };
            let suffix = if table.is_view { "view" } else { "schema" };
            archive.write(
                format!("{path}/{}.{}.{suffix}.txt", table.database, table.table),
                create,
            )?;
            if let Some((_, info)) = self.domain.stats_table(&table.database, &table.table) {
                if let Some(replica) = info.TiFlashReplica {
                    replicas.push_str(&format!(
                        "{}.{}: {}\n",
                        table.database, table.table, replica.Count
                    ));
                }
            }
            if !task.skip_stats && !table.is_view {
                let mut stats = serde_json::Map::new();
                for kind in ["META", "HISTOGRAMS", "BUCKETS", "TOPN"] {
                    let rows = sql_rows(&self.domain, &format!("SHOW STATS_{kind}"))?
                        .into_iter()
                        .filter(|row| {
                            row.first()
                                .is_some_and(|name| name.eq_ignore_ascii_case(&table.database))
                                && row
                                    .get(1)
                                    .is_some_and(|name| name.eq_ignore_ascii_case(&table.table))
                        })
                        .collect::<Vec<_>>();
                    stats.insert(kind.to_owned(), serde_json::json!(rows));
                }
                archive.write(
                    format!("stats/{}.{}.json", table.database, table.table),
                    serde_json::to_vec(&stats).map_err(|error| error.to_string())?,
                )?;
            }
        }
        archive.write("table_tiflash_replica.txt", replicas)?;
        for record in package.records.values() {
            let path = if record.skipped {
                "skippedSQLs"
            } else {
                "SQLs"
            };
            archive.write(
                format!("{path}/{}.json", record.digest),
                serde_json::to_vec(&serde_json::json!({
                    "schema": record.schema_name,
                    "plan": record.decoded_plan,
                    "sql": record.sql,
                    "digest": record.digest,
                    "binaryPlan": record.binary_plan,
                    "userName": record.user_name,
                }))
                .map_err(|error| error.to_string())?,
            )?;
        }
        let bytes = encode_replay_archive(&archive)?;
        let context = StorageContext::background();
        GetGlobalExtStorage(&context)
            .map_err(|error| error.to_string())?
            .WriteFile(
                &context,
                &format!("{EXTRACT_DIRECTORY}/{file_name}"),
                &bytes,
            )
            .map_err(|error| error.to_string())
    }

    fn persistent_statement_summary_enabled(&self) -> bool {
        sql_rows(&self.domain, "SELECT @@tidb_stmt_summary_enable_persistent")
            .ok()
            .and_then(|rows| rows.first().and_then(|row| row.first()).cloned())
            .is_some_and(|value| value.eq_ignore_ascii_case("ON") || value == "1")
    }
}

struct StorageExtractReader(Box<dyn astersql_planner_extstore::objstore::storage::ObjectReader>);

impl ExtractReader for StorageExtractReader {
    fn read(&mut self, buffer: &mut [u8]) -> ExtractResult<usize> {
        self.0
            .read(buffer)
            .map_err(|error| ExtractError(error.to_string()))
    }

    fn close(&mut self) -> ExtractResult<()> {
        self.0
            .close()
            .map_err(|error| ExtractError(error.to_string()))
    }
}

pub(crate) struct CanonicalExtractRuntime {
    handle: ExtractHandle,
}

impl CanonicalExtractRuntime {
    pub(crate) fn new(domain: Arc<Domain>) -> Self {
        let source: Arc<dyn ExtractSource> = Arc::new(ProductionExtractSource {
            domain: Arc::clone(&domain),
        });
        Self {
            handle: ExtractHandle::new_with_domain(domain, source),
        }
    }
}

impl ExtractRuntime for CanonicalExtractRuntime {
    fn now(&self) -> Timestamp {
        Timestamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        )
    }

    fn parse_time(&self, value: &str) -> ExtractResult<Timestamp> {
        chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
            .map(|value| Timestamp(value.and_utc().timestamp()))
            .map_err(|error| ExtractError(error.to_string()))
    }

    fn extract_task(&self, context: &RequestContext, task: ExtractTask) -> ExtractResult<String> {
        if context.cancelled {
            return Err(ExtractError("extract task canceled".into()));
        }
        let begin = UNIX_EPOCH + Duration::from_secs(task.begin.0.max(0) as u64);
        let end = UNIX_EPOCH + Duration::from_secs(task.end.0.max(0) as u64);
        let mut domain_task = DomainExtractTask::new_plan(begin, end);
        domain_task.is_background_job = task.is_background_job;
        domain_task.skip_stats = task.skip_stats;
        domain_task.use_history_view = task.use_history_view;
        Ok(self
            .handle
            .extract_task(&domain_task)
            .map_err(ExtractError)?
            .unwrap_or_default())
    }

    fn extract_task_directory(&self) -> String {
        EXTRACT_DIRECTORY.into()
    }

    fn open_extract(
        &self,
        context: &RequestContext,
        path: &str,
    ) -> ExtractResult<Box<dyn ExtractReader>> {
        if context.cancelled {
            return Err(ExtractError("extract read canceled".into()));
        }
        let storage_context = StorageContext::background();
        let reader = GetGlobalExtStorage(&storage_context)
            .map_err(|error| ExtractError(error.to_string()))?
            .Open(&storage_context, path, None)
            .map_err(|error| ExtractError(error.to_string()))?;
        Ok(Box::new(StorageExtractReader(reader)))
    }

    fn failpoint_enabled(&self, _name: &str) -> bool {
        false
    }
    fn log_error(&self, message: &str, error: &ExtractError) {
        eprintln!("{message}: {error}");
    }
    fn log_warning(&self, message: &str, error: &ExtractError) {
        eprintln!("{message}: {error}");
    }
}
